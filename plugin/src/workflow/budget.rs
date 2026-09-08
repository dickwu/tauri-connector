//! Process-local retention accounting. Reservations are owned by the data they
//! cover and release their charge when the run or temporary value is dropped.
use connector_client::outcome::WorkflowError;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub const DEFAULT_MEMORY_LIMIT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct MemoryBudget(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    used: AtomicUsize,
    limit: usize,
}

/// Deliberately not Clone: each byte charge has exactly one releasing owner.
#[derive(Debug)]
pub struct Reservation {
    budget: MemoryBudget,
    bytes: usize,
}

impl Default for MemoryBudget {
    fn default() -> Self {
        Self::new(DEFAULT_MEMORY_LIMIT_BYTES)
    }
}

impl MemoryBudget {
    pub fn new(limit_bytes: usize) -> Self {
        Self(Arc::new(Inner {
            used: AtomicUsize::new(0),
            limit: limit_bytes,
        }))
    }

    pub fn limit_bytes(&self) -> usize {
        self.0.limit
    }
    pub fn used_bytes(&self) -> usize {
        self.0.used.load(Ordering::Acquire)
    }

    fn charge(&self, bytes: usize) -> Result<(), WorkflowError> {
        let mut used = self.0.used.load(Ordering::Acquire);
        loop {
            // Subtraction and comparison avoid integer wrap even with a host
            // limit of usize::MAX. Failed requests never mutate accounting.
            if bytes > self.0.limit.saturating_sub(used) {
                return Err(exhausted());
            }
            match self.0.used.compare_exchange_weak(
                used,
                used + bytes,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(current) => used = current,
            }
        }
    }

    pub fn try_reserve(&self, bytes: usize) -> Result<Reservation, WorkflowError> {
        self.charge(bytes)?;
        Ok(Reservation {
            budget: self.clone(),
            bytes,
        })
    }
}

impl Reservation {
    #[cfg(test)]
    pub fn reserved_bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    pub fn grow(&mut self, additional_bytes: usize) -> Result<(), WorkflowError> {
        let total = self
            .bytes
            .checked_add(additional_bytes)
            .ok_or_else(exhausted)?;
        self.budget.charge(additional_bytes)?;
        self.bytes = total;
        Ok(())
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let previous = self.budget.0.used.fetch_sub(self.bytes, Ordering::AcqRel);
        debug_assert!(
            previous >= self.bytes,
            "Memory reservation accounting underflow"
        );
    }
}

fn exhausted() -> WorkflowError {
    WorkflowError::new(
        "resource_busy",
        "retaining",
        "Workflow retention memory budget is exhausted",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reservation_releases_exact_charge_on_drop() {
        let budget = MemoryBudget::new(100);
        let first = budget.try_reserve(60).unwrap();
        let second = budget.try_reserve(40).unwrap();
        assert_eq!(budget.used_bytes(), 100);
        assert_eq!(budget.try_reserve(1).unwrap_err().code, "resource_busy");
        drop(first);
        assert_eq!(budget.used_bytes(), 40);
        drop(second);
        assert_eq!(budget.used_bytes(), 0);
    }
    #[test]
    fn failed_grow_does_not_change_any_charge() {
        let budget = MemoryBudget::new(100);
        let mut reservation = budget.try_reserve(60).unwrap();
        assert!(reservation.grow(41).is_err());
        assert_eq!(reservation.reserved_bytes(), 60);
        assert_eq!(budget.used_bytes(), 60);
        reservation.grow(40).unwrap();
        assert_eq!(reservation.reserved_bytes(), 100);
        drop(reservation);
        assert_eq!(budget.used_bytes(), 0);
    }
    #[test]
    fn zero_and_overflow_do_not_wrap_accounting() {
        let zero = MemoryBudget::new(0);
        let empty = zero.try_reserve(0).unwrap();
        assert!(zero.try_reserve(1).is_err());
        drop(empty);
        let budget = MemoryBudget::new(usize::MAX);
        let mut huge = budget.try_reserve(usize::MAX).unwrap();
        assert!(huge.grow(1).is_err());
        assert_eq!(huge.reserved_bytes(), usize::MAX);
        assert!(budget.try_reserve(1).is_err());
        drop(huge);
        assert_eq!(budget.used_bytes(), 0);
    }
    #[test]
    fn clones_share_one_atomic_limit_under_contention() {
        let budget = MemoryBudget::new(64);
        let barrier = Arc::new(std::sync::Barrier::new(17));
        let mut threads = Vec::new();
        for _ in 0..16 {
            let budget = budget.clone();
            let barrier = barrier.clone();
            threads.push(std::thread::spawn(move || {
                let reservation = budget.try_reserve(8).ok();
                barrier.wait();
                barrier.wait();
                reservation.is_some()
            }));
        }
        barrier.wait();
        assert_eq!(budget.used_bytes(), 64);
        barrier.wait();
        assert_eq!(
            threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .filter(|held| *held)
                .count(),
            8
        );
        assert_eq!(budget.used_bytes(), 0);
    }
    #[test]
    fn default_has_a_fixed_host_limit() {
        assert_eq!(MemoryBudget::default().limit_bytes(), 64 * 1024 * 1024);
    }
}
