use super::{PoolRecord, PoolRef};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

#[test]
fn a_borrowed_bus_cannot_be_freed_by_a_reentrant_operation() {
    let mut record = PoolRecord::empty();
    let pool = PoolRef::from(&mut record);
    let owner = Rc::new(());
    pool.set_message_bus(Some(Box::new(owner.clone())));
    assert_eq!(Rc::strong_count(&owner), 2);
    let reentered = catch_unwind(AssertUnwindSafe(|| {
        pool.with_message_bus(|bus| {
            assert!(bus.is::<Rc<()>>());
            pool.set_message_bus(None);
        });
    }));
    assert!(reentered.is_err());
    assert!(pool.has_message_bus());
    assert_eq!(Rc::strong_count(&owner), 2);
    pool.set_message_bus(None);
    assert!(!pool.has_message_bus());
    assert_eq!(Rc::strong_count(&owner), 1);
}
