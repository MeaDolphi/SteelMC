//! The auction house: escrowed lots, timed bidding, and one ledger.
//!
//! A listed stack leaves its owner's inventory and lives in this ledger until
//! the lot resolves; a bid's money does the same. Everything owed back to a
//! player — an unsold stack, a refunded bid, sale revenue — becomes a delivery
//! in the same ledger, so nothing waits "somewhere in between" and a restart
//! cannot lose a lot, a deadline, or a settled sale.

use std::mem;

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Active lots one player may hold, counting both kinds together.
pub const MAX_ACTIVE_LOTS: usize = 10;
/// Shortest lot duration, in whole hours.
pub const MIN_DURATION_HOURS: u64 = 1;
/// Longest lot duration, in whole hours.
pub const MAX_DURATION_HOURS: u64 = 24;
/// Seconds in one hour, the unit lot durations are given in.
pub const HOUR_SECONDS: u64 = 60 * 60;
/// Commission taken from a successful sale, in percent.
pub const COMMISSION_PERCENT: u64 = 5;

/// How a lot accepts buyers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LotKind {
    /// The first buyer to pay the price wins, and the lot closes immediately.
    FixedPrice,
    /// Bidders raise each other until the deadline, then the highest wins.
    Bidding,
}

/// The standing bid on a bidding lot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bid {
    /// Hunter whose money is escrowed for this bid.
    pub bidder: Uuid,
    /// Escrowed amount, which is also the sale price if the lot expires here.
    pub amount: u64,
}

/// One listed stack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lot {
    /// Stable identifier shown to players and used by every lot command.
    pub id: u64,
    /// Hunter who receives the revenue.
    pub seller: Uuid,
    /// Seller name at listing time, so the lot reads the same after a restart.
    pub seller_name: String,
    /// NBT-encoded escrowed stack.
    pub item: Vec<u8>,
    /// How the lot accepts buyers.
    pub kind: LotKind,
    /// Fixed price, or the lowest acceptable first bid.
    pub price: u64,
    /// Unix seconds at which the lot resolves without a buyer.
    pub expires_at: u64,
    /// Standing bid of a bidding lot.
    #[serde(default)]
    pub bid: Option<Bid>,
}

impl Lot {
    /// Returns the price a sale at this instant would raise.
    #[must_use]
    pub fn winning_price(&self) -> u64 {
        self.bid.map_or(self.price, |bid| bid.amount)
    }

    /// Returns the lowest bid this lot accepts right now.
    #[must_use]
    pub fn minimum_bid(&self) -> u64 {
        self.bid
            .map_or(self.price, |bid| bid.amount.saturating_add(1))
    }
}

/// Money and stacks the ledger still owes one player.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    /// Won to add to the balance.
    #[serde(default)]
    pub won: u64,
    /// NBT-encoded stacks to hand back or forward.
    #[serde(default)]
    pub items: Vec<Vec<u8>>,
}

impl Delivery {
    /// Returns whether there is nothing left to deliver.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.won == 0 && self.items.is_empty()
    }
}

/// Every lot and every unsettled delivery, persisted as one document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ledger {
    /// Identifier the next listed lot receives.
    #[serde(default)]
    next_id: u64,
    /// Active lots in listing order.
    #[serde(default)]
    lots: Vec<Lot>,
    /// Unsettled deliveries per hunter.
    #[serde(default)]
    deliveries: FxHashMap<Uuid, Delivery>,
}

/// Why the ledger refused a lot operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuctionError {
    /// No lot uses that identifier, or it already resolved.
    UnknownLot,
    /// A hunter may not trade with their own lot.
    OwnLot,
    /// Only the seller may cancel this lot.
    NotSeller,
    /// The lot passed its deadline and is waiting to resolve.
    Expired,
    /// A fixed-price lot cannot be bid on.
    NotForBidding,
    /// A bidding lot cannot be bought outright.
    NotForSale,
    /// A lot with a standing bid can no longer be withdrawn.
    BidsExist,
    /// The seller already holds the maximum number of active lots.
    LotLimit,
    /// Prices and bids start at one Won.
    InvalidPrice,
    /// Durations are whole hours within the allowed range.
    InvalidDuration,
    /// The bid does not reach the lowest accepted amount.
    BidTooLow,
}

impl Ledger {
    /// Returns every active lot, in listing order.
    #[must_use]
    pub fn lots(&self) -> &[Lot] {
        &self.lots
    }

    /// Returns one active lot.
    #[must_use]
    pub fn lot(&self, id: u64) -> Option<&Lot> {
        self.lots.iter().find(|lot| lot.id == id)
    }

    /// Returns how many active lots a hunter holds.
    #[must_use]
    pub fn active_lots(&self, seller: Uuid) -> usize {
        self.lots.iter().filter(|lot| lot.seller == seller).count()
    }

    /// Returns what the ledger still owes a hunter.
    #[must_use]
    pub fn delivery(&self, hunter: Uuid) -> Option<&Delivery> {
        self.deliveries.get(&hunter)
    }

    /// Returns every hunter with an unsettled delivery.
    pub fn delivery_owners(&self) -> impl Iterator<Item = Uuid> + '_ {
        self.deliveries.keys().copied()
    }

    /// Withdraws a hunter's settled delivery, returning `None` when empty.
    pub fn take_delivery(&mut self, hunter: Uuid) -> Option<Delivery> {
        self.deliveries.remove(&hunter).filter(|d| !d.is_empty())
    }

    /// Escrows a stack and lists it for `hours`.
    #[expect(
        clippy::too_many_arguments,
        reason = "a lot is fully described by its escrow, terms, and deadline"
    )]
    pub fn list(
        &mut self,
        seller: Uuid,
        seller_name: &str,
        item: Vec<u8>,
        kind: LotKind,
        price: u64,
        hours: u64,
        now: u64,
    ) -> Result<u64, AuctionError> {
        if price == 0 {
            return Err(AuctionError::InvalidPrice);
        }
        if !(MIN_DURATION_HOURS..=MAX_DURATION_HOURS).contains(&hours) {
            return Err(AuctionError::InvalidDuration);
        }
        if self.active_lots(seller) >= MAX_ACTIVE_LOTS {
            return Err(AuctionError::LotLimit);
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.lots.push(Lot {
            id,
            seller,
            seller_name: seller_name.to_owned(),
            item,
            kind,
            price,
            expires_at: now.saturating_add(hours.saturating_mul(HOUR_SECONDS)),
            bid: None,
        });
        Ok(id)
    }

    /// Sells a fixed-price lot immediately and returns the price to charge.
    pub fn buy(&mut self, id: u64, buyer: Uuid, now: u64) -> Result<u64, AuctionError> {
        let index = self.index(id)?;
        let lot = &self.lots[index];
        if lot.seller == buyer {
            return Err(AuctionError::OwnLot);
        }
        if lot.kind != LotKind::FixedPrice {
            return Err(AuctionError::NotForSale);
        }
        if lot.expires_at <= now {
            return Err(AuctionError::Expired);
        }
        let price = lot.price;
        let lot = self.lots.remove(index);
        self.sell(lot, buyer);
        Ok(price)
    }

    /// Places a bid and returns the amount to charge the bidder.
    ///
    /// The previous bidder is refunded through a delivery, so an outbid hunter
    /// loses nothing even when they are offline by the time the lot resolves.
    pub fn bid(
        &mut self,
        id: u64,
        bidder: Uuid,
        amount: u64,
        now: u64,
    ) -> Result<u64, AuctionError> {
        let index = self.index(id)?;
        let previous = {
            let lot = &mut self.lots[index];
            if lot.seller == bidder {
                return Err(AuctionError::OwnLot);
            }
            if lot.kind != LotKind::Bidding {
                return Err(AuctionError::NotForBidding);
            }
            if lot.expires_at <= now {
                return Err(AuctionError::Expired);
            }
            if amount < lot.minimum_bid() {
                return Err(AuctionError::BidTooLow);
            }
            lot.bid.replace(Bid { bidder, amount })
        };
        if let Some(previous) = previous {
            self.deliver_won(previous.bidder, previous.amount);
        }
        Ok(amount)
    }

    /// Withdraws a lot that nobody has bid on.
    pub fn cancel(&mut self, id: u64, seller: Uuid) -> Result<(), AuctionError> {
        let index = self.index(id)?;
        let lot = &self.lots[index];
        if lot.seller != seller {
            return Err(AuctionError::NotSeller);
        }
        if lot.bid.is_some() {
            return Err(AuctionError::BidsExist);
        }
        let lot = self.lots.remove(index);
        self.deliver_item(seller, lot.item);
        Ok(())
    }

    /// Resolves every lot whose deadline has passed and returns how many did.
    ///
    /// A lot with a bid sells to its bidder at the escrowed amount; a lot
    /// without one returns its stack to the seller. Neither gains extra time.
    pub fn expire(&mut self, now: u64) -> usize {
        let (expired, active) = mem::take(&mut self.lots)
            .into_iter()
            .partition(|lot| lot.expires_at <= now);
        self.lots = active;
        let resolved = expired.len();
        for lot in expired {
            match lot.bid {
                Some(bid) => self.sell(lot, bid.bidder),
                None => self.deliver_item(lot.seller, lot.item),
            }
        }
        resolved
    }

    fn index(&self, id: u64) -> Result<usize, AuctionError> {
        self.lots
            .iter()
            .position(|lot| lot.id == id)
            .ok_or(AuctionError::UnknownLot)
    }

    /// Hands the escrowed stack and the net revenue out, ending the lot.
    fn sell(&mut self, lot: Lot, winner: Uuid) {
        let price = lot.winning_price();
        self.deliver_item(winner, lot.item);
        self.deliver_won(lot.seller, price.saturating_sub(commission(price)));
    }

    fn deliver_won(&mut self, hunter: Uuid, amount: u64) {
        if amount == 0 {
            return;
        }
        let delivery = self.deliveries.entry(hunter).or_default();
        delivery.won = delivery.won.saturating_add(amount);
    }

    fn deliver_item(&mut self, hunter: Uuid, item: Vec<u8>) {
        self.deliveries.entry(hunter).or_default().items.push(item);
    }
}

/// Returns the commission on a sale, rounded up and never above the price.
#[must_use]
pub const fn commission(price: u64) -> u64 {
    let raw = price.saturating_mul(COMMISSION_PERCENT);
    let mut rounded_up = raw / 100;
    if !raw.is_multiple_of(100) {
        rounded_up += 1;
    }
    if rounded_up > price {
        price
    } else {
        rounded_up
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    fn listed(ledger: &mut Ledger, seller: Uuid, kind: LotKind, price: u64) -> u64 {
        ledger
            .list(seller, "MeaDolphi", vec![7, 7, 7], kind, price, 1, NOW)
            .expect("listing should be accepted")
    }

    #[test]
    fn commission_rounds_up_and_never_exceeds_the_price() {
        assert_eq!(commission(0), 0);
        assert_eq!(commission(1), 1);
        assert_eq!(commission(20), 1);
        assert_eq!(commission(21), 2);
        assert_eq!(commission(100), 5);
        assert_eq!(commission(2_000), 100);
    }

    #[test]
    fn a_listed_lot_keeps_the_stack_and_respects_limits() {
        let seller = Uuid::new_v4();
        let mut ledger = Ledger::default();
        assert_eq!(
            ledger.list(seller, "x", vec![0], LotKind::FixedPrice, 0, 1, NOW),
            Err(AuctionError::InvalidPrice)
        );
        for hours in [0, MAX_DURATION_HOURS + 1] {
            assert_eq!(
                ledger.list(seller, "x", vec![0], LotKind::FixedPrice, 1, hours, NOW),
                Err(AuctionError::InvalidDuration)
            );
        }

        let id = ledger
            .list(
                seller,
                "x",
                vec![0],
                LotKind::Bidding,
                5,
                MAX_DURATION_HOURS,
                NOW,
            )
            .expect("a full-day lot is allowed");
        let lot = ledger.lot(id).expect("lot should exist");
        assert_eq!(lot.expires_at, NOW + MAX_DURATION_HOURS * HOUR_SECONDS);

        for _ in 1..MAX_ACTIVE_LOTS {
            listed(&mut ledger, seller, LotKind::FixedPrice, 10);
        }
        assert_eq!(ledger.active_lots(seller), MAX_ACTIVE_LOTS);
        assert_eq!(
            ledger.list(seller, "x", vec![0], LotKind::FixedPrice, 1, 1, NOW),
            Err(AuctionError::LotLimit)
        );
        assert!(ledger.lots().iter().all(|lot| lot.id < ledger.next_id));
    }

    #[test]
    fn a_fixed_price_sale_pays_the_seller_net_of_commission() {
        let seller = Uuid::new_v4();
        let buyer = Uuid::new_v4();
        let mut ledger = Ledger::default();
        let id = listed(&mut ledger, seller, LotKind::FixedPrice, 100);

        assert_eq!(ledger.buy(id, seller, NOW), Err(AuctionError::OwnLot));
        assert_eq!(ledger.buy(id, buyer, NOW), Ok(100));
        assert!(ledger.lots().is_empty());
        assert_eq!(ledger.delivery(seller).map(|d| d.won), Some(95));
        assert_eq!(
            ledger.delivery(buyer).map(|d| d.items.clone()),
            Some(vec![vec![7, 7, 7]])
        );
        assert_eq!(ledger.buy(id, buyer, NOW), Err(AuctionError::UnknownLot));
    }

    #[test]
    fn bidding_raises_the_standing_bid_and_refunds_the_previous_bidder() {
        let seller = Uuid::new_v4();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let mut ledger = Ledger::default();
        let id = listed(&mut ledger, seller, LotKind::Bidding, 50);

        assert_eq!(ledger.bid(id, first, 49, NOW), Err(AuctionError::BidTooLow));
        assert_eq!(ledger.bid(id, first, 50, NOW), Ok(50));
        assert_eq!(
            ledger.bid(id, second, 50, NOW),
            Err(AuctionError::BidTooLow)
        );
        assert_eq!(ledger.bid(id, second, 51, NOW), Ok(51));
        assert_eq!(ledger.delivery(first).map(|d| d.won), Some(50));
        assert_eq!(
            ledger.lot(id).and_then(|lot| lot.bid),
            Some(Bid {
                bidder: second,
                amount: 51
            })
        );
        assert_eq!(ledger.buy(id, second, NOW), Err(AuctionError::NotForSale));
        assert_eq!(ledger.bid(id, seller, 99, NOW), Err(AuctionError::OwnLot));
    }

    #[test]
    fn an_expired_lot_sells_to_its_bidder_or_returns_to_the_seller() {
        let seller = Uuid::new_v4();
        let bidder = Uuid::new_v4();
        let mut ledger = Ledger::default();
        let sold = listed(&mut ledger, seller, LotKind::Bidding, 10);
        let returned = listed(&mut ledger, seller, LotKind::Bidding, 10);
        ledger.bid(sold, bidder, 40, NOW).expect("bid");

        assert_eq!(ledger.expire(NOW + HOUR_SECONDS - 1), 0);
        assert_eq!(ledger.expire(NOW + HOUR_SECONDS), 2);
        assert!(ledger.lots().is_empty());
        assert_eq!(
            ledger.delivery(bidder).map(|d| d.items.clone()),
            Some(vec![vec![7, 7, 7]])
        );
        let seller_delivery = ledger.delivery(seller).expect("seller delivery");
        assert_eq!(seller_delivery.won, 38);
        assert_eq!(seller_delivery.items, vec![vec![7, 7, 7]]);
        assert_eq!(
            ledger.bid(returned, bidder, 40, NOW),
            Err(AuctionError::UnknownLot)
        );
    }

    #[test]
    fn a_lot_with_a_bid_can_no_longer_be_withdrawn() {
        let seller = Uuid::new_v4();
        let bidder = Uuid::new_v4();
        let mut ledger = Ledger::default();
        let id = listed(&mut ledger, seller, LotKind::Bidding, 10);

        assert_eq!(ledger.cancel(id, bidder), Err(AuctionError::NotSeller));
        ledger.bid(id, bidder, 10, NOW).expect("bid");
        assert_eq!(ledger.cancel(id, seller), Err(AuctionError::BidsExist));

        let quiet = listed(&mut ledger, seller, LotKind::Bidding, 10);
        ledger
            .cancel(quiet, seller)
            .expect("cancelling an unbid lot");
        assert_eq!(
            ledger.delivery(seller).map(|d| d.items.len()),
            Some(1),
            "the stack returns to the seller"
        );
    }

    #[test]
    fn taking_a_delivery_leaves_the_ledger_empty_for_that_hunter() {
        let seller = Uuid::new_v4();
        let mut ledger = Ledger::default();
        let id = listed(&mut ledger, seller, LotKind::FixedPrice, 100);
        ledger.buy(id, Uuid::new_v4(), NOW).expect("sale");

        let delivery = ledger.take_delivery(seller).expect("settled delivery");
        assert_eq!(delivery.won, 95);
        assert!(ledger.take_delivery(seller).is_none());
        assert_eq!(ledger.delivery_owners().count(), 1);
    }

    #[test]
    fn a_ledger_round_trips_through_json() {
        let seller = Uuid::new_v4();
        let mut ledger = Ledger::default();
        let id = listed(&mut ledger, seller, LotKind::Bidding, 10);
        ledger.bid(id, Uuid::new_v4(), 25, NOW).expect("bid");

        let bytes = serde_json::to_vec(&ledger).expect("serialize");
        let restored: Ledger = serde_json::from_slice(&bytes).expect("deserialize");
        let lot = restored.lot(id).expect("lot survives the round trip");
        assert_eq!(lot.seller_name, "MeaDolphi");
        assert_eq!(lot.bid.map(|bid| bid.amount), Some(25));
    }
}
