use core::cell::Cell;
use core::future::Future;

pub(crate) async fn abandoned_entry(
    timer: impl Future<Output = ()>,
    download_started: &Cell<bool>,
) {
    timer.await;
    if download_started.get() {
        core::future::pending::<()>().await;
    }
}

pub(crate) fn valid_vectors(bank: core::ops::Range<usize>, sp: u32, reset: u32) -> bool {
    (0x2000_0000..=0x2008_0000).contains(&sp)
        && sp.is_multiple_of(8)
        && reset & 1 == 1
        && bank.contains(&((reset & !1) as usize))
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;
    use core::future::{Future, pending, ready};
    use core::pin::pin;
    use core::task::{Context, Poll, Waker};

    use super::{abandoned_entry, valid_vectors};

    #[test]
    fn timeout_only_completes_without_a_download() {
        let flag = Cell::new(false);
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(
            pin!(abandoned_entry(pending(), &flag)).poll(&mut context),
            Poll::Pending
        );
        assert_eq!(
            pin!(abandoned_entry(ready(()), &flag)).poll(&mut context),
            Poll::Ready(())
        );
        flag.set(true);
        assert_eq!(
            pin!(abandoned_entry(ready(()), &flag)).poll(&mut context),
            Poll::Pending
        );
    }

    #[test]
    fn resume_requires_ram_stack_and_thumb_entry_inside_selected_bank() {
        for (sp, reset, expected) in [
            (0x2008_0000, 0x10001, true),
            (0x2000_0000, 0x1ffff, true),
            (0x1fff_fff8, 0x10001, false),
            (0x2008_0008, 0x10001, false),
            (0x2000_0004, 0x10001, false),
            (0x2008_0000, 0x10000, false),
            (0x2008_0000, 0x0ffff, false),
            (0x2008_0000, 0x20001, false),
            (u32::MAX, u32::MAX, false),
        ] {
            assert_eq!(
                valid_vectors(0x10000..0x20000, sp, reset),
                expected,
                "sp={sp:#x}, reset={reset:#x}"
            );
        }
    }
}
