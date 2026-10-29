//! Non-cancellable durable inventory work, completed at a server job boundary.

use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, mpsc},
};

use tokio::task::JoinHandle;

use super::{JobPoll, ServerJob, ServerJobContext};
use crate::{
    player::{
        inventory_transaction::StoredInventoryReservation,
        player_data_storage::transaction::{
            ExtensionStorageWrite, PlayerStorageTransaction, TransactionCommit,
        },
    },
    server::Server,
};

type Completion = Box<dyn FnOnce(io::Result<TransactionCommit>) + Send>;

struct InventoryTransactionJob {
    reservations: Vec<StoredInventoryReservation>,
    receiver: mpsc::Receiver<io::Result<TransactionCommit>>,
    task: Option<JoinHandle<()>>,
    completion: Option<Completion>,
}

impl Server {
    /// Starts I/O without blocking a tick. Reservations and completion are owned
    /// until the result is installed, including disconnect and graceful shutdown.
    /// Call from a job safe point, not while a menu callback owns inventory locks.
    pub fn submit_stored_inventory_transaction(
        self: &Arc<Self>,
        reservations: Vec<StoredInventoryReservation>,
        extensions: Vec<ExtensionStorageWrite>,
        completion: impl FnOnce(io::Result<TransactionCommit>) + Send + 'static,
    ) -> io::Result<()> {
        if reservations.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no inventory reservations",
            ));
        }
        let transaction = PlayerStorageTransaction {
            players: reservations
                .iter()
                .map(StoredInventoryReservation::storage_write)
                .collect(),
            extensions,
        };
        let (sender, receiver) = mpsc::channel();
        let server = Arc::clone(self);
        let task = tokio::spawn(async move {
            let result = server
                .player_data_storage
                .commit_transaction(&transaction)
                .await;
            let _ = sender.send(result);
        });
        self.jobs.spawn(InventoryTransactionJob {
            reservations,
            receiver,
            task: Some(task),
            completion: Some(Box::new(completion)),
        });
        Ok(())
    }
}

impl InventoryTransactionJob {
    fn finish(&mut self, result: io::Result<TransactionCommit>) {
        if result.is_ok() {
            for reservation in &mut self.reservations {
                reservation.install();
            }
        }
        if let Some(completion) = self.completion.take() {
            completion(result);
        }
        // Releasing can resume deferred death drops/respawn, after both sides
        // of the transaction (inventory and gameplay-owned state) are installed.
        self.reservations.clear();
    }
}

impl ServerJob for InventoryTransactionJob {
    fn poll(&mut self, _context: &mut ServerJobContext) -> JobPoll {
        match self.receiver.try_recv() {
            Ok(result) => {
                self.finish(result);
                JobPoll::Finished
            }
            Err(mpsc::TryRecvError::Empty) => JobPoll::Pending,
            Err(mpsc::TryRecvError::Disconnected) => {
                // A panic in the storage task leaves commit status unknown.
                // Continuing with stale live state could duplicate items.
                panic!("durable inventory task terminated without a commit result");
            }
        }
    }

    fn is_cancellable(&self) -> bool {
        false
    }

    fn finish_shutdown(mut self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            if let Some(task) = self.task.take()
                && let Err(error) = task.await
            {
                panic!("durable inventory task failed during shutdown: {error}");
            }
            match self.receiver.try_recv() {
                Ok(result) => self.finish(result),
                Err(error) => panic!("durable inventory task lost its shutdown result: {error}"),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        inventory::container::Container,
        player::player_data_storage::PlayerDataStorage,
        test_support::{TestPlayerBuilder, fresh_test_world},
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use steel_registry::{item_stack::ItemStack, vanilla_items};
    use steel_utils::Identifier;
    use tokio::{runtime::Builder, sync::oneshot};

    #[test]
    fn storage_wait_never_blocks_a_job_tick_and_shutdown_finishes_the_commit() {
        let fixture = fresh_test_world("inventory_transaction_job");
        let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "Buyer", 1).build();
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let mut reservation = player.reserve_stored_inventory().expect("reserve");
            reservation
                .set_item(9, ItemStack::new(&vanilla_items::DIAMOND))
                .expect("stage");
            let storage = Arc::new(PlayerDataStorage::in_memory());
            let transaction = PlayerStorageTransaction {
                players: vec![reservation.storage_write()],
                extensions: vec![ExtensionStorageWrite {
                    key: Identifier::new("test", "payment"),
                    expected: None,
                    replacement: b"paid".to_vec(),
                }],
            };
            let (release, gate) = oneshot::channel();
            let (sender, receiver) = mpsc::channel();
            let task_storage = Arc::clone(&storage);
            let task = tokio::spawn(async move {
                gate.await.expect("release storage");
                let result = task_storage.commit_transaction(&transaction).await;
                sender.send(result).expect("live job");
            });
            let called = Arc::new(AtomicBool::new(false));
            let callback_called = Arc::clone(&called);
            let mut job = InventoryTransactionJob {
                reservations: vec![reservation],
                receiver,
                task: Some(task),
                completion: Some(Box::new(move |result| {
                    assert_eq!(result.expect("commit"), TransactionCommit::Committed);
                    callback_called.store(true, Ordering::Release);
                })),
            };
            let mut context = ServerJobContext::for_test(1, true);
            assert_eq!(job.poll(&mut context), JobPoll::Pending);
            assert!(player.inventory.lock().get_item(9).is_empty());
            assert!(!called.load(Ordering::Acquire));
            let queue = super::super::ServerJobQueue::new();
            queue.spawn(job);
            queue.cancel_all();
            assert_eq!(queue.len(), 1, "durable jobs cannot be cancelled");
            release.send(()).expect("release");
            queue.finish_all().await;
            assert!(queue.is_empty());
            assert!(called.load(Ordering::Acquire));
            assert!(!player.inventory_transaction_active());
            assert_eq!(
                player.inventory.lock().get_item(9).item(),
                &*vanilla_items::DIAMOND
            );
            assert_eq!(
                storage
                    .load_extension(&Identifier::new("test", "payment"))
                    .await
                    .expect("document"),
                Some(b"paid".to_vec())
            );
        });
    }
}
