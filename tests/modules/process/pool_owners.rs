use super::{PoolRecord, PoolRef};
use std::rc::Rc;

#[test]
fn a_borrowed_bus_cannot_be_freed_by_a_reentrant_operation() {
    let mut record = PoolRecord::empty();
    let pool = PoolRef::from(&mut record);
    let owner = Rc::new(());
    pool.set_message_bus(Some(Box::new(owner.clone())));
    assert_eq!(Rc::strong_count(&owner), 2);
    pool.with_message_bus(|bus| {
        assert!(bus.is::<Rc<()>>());
        // The lent bus is out of the pool: a reentrant operation finds none to free.
        assert!(!pool.has_message_bus());
        pool.set_message_bus(None);
        assert_eq!(Rc::strong_count(&owner), 2);
    });
    assert!(pool.has_message_bus());
    assert_eq!(Rc::strong_count(&owner), 2);
    pool.set_message_bus(None);
    assert!(!pool.has_message_bus());
    assert_eq!(Rc::strong_count(&owner), 1);
}
