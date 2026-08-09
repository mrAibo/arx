use std::future::Future;
use std::io;

use crossterm::{
    cursor::Show,
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};

pub(crate) trait TerminalOps {
    fn enable_raw(&mut self) -> io::Result<()>;
    fn disable_raw(&mut self) -> io::Result<()>;
    fn enter_alternate(&mut self) -> io::Result<()>;
    fn leave_alternate(&mut self) -> io::Result<()>;
    fn enable_mouse(&mut self) -> io::Result<()>;
    fn disable_mouse(&mut self) -> io::Result<()>;
    fn show_cursor(&mut self) -> io::Result<()>;
}

#[derive(Debug, Default)]
pub(crate) struct CrosstermTerminalOps;

impl TerminalOps for CrosstermTerminalOps {
    fn enable_raw(&mut self) -> io::Result<()> {
        enable_raw_mode()
    }

    fn disable_raw(&mut self) -> io::Result<()> {
        disable_raw_mode()
    }

    fn enter_alternate(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnterAlternateScreen)
    }

    fn leave_alternate(&mut self) -> io::Result<()> {
        execute!(io::stdout(), LeaveAlternateScreen)
    }

    fn enable_mouse(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnableMouseCapture)
    }

    fn disable_mouse(&mut self) -> io::Result<()> {
        execute!(io::stdout(), DisableMouseCapture)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(io::stdout(), Show)
    }
}

pub(crate) struct TuiTerminalSession<O: TerminalOps = CrosstermTerminalOps> {
    ops: O,
    raw_active: bool,
    alternate_active: bool,
    mouse_active: bool,
    cursor_restore_pending: bool,
}

impl TuiTerminalSession<CrosstermTerminalOps> {
    pub(crate) fn enter() -> io::Result<Self> {
        Self::enter_with(CrosstermTerminalOps)
    }
}

impl<O: TerminalOps> TuiTerminalSession<O> {
    fn enter_with(ops: O) -> io::Result<Self> {
        let mut session = Self {
            ops,
            raw_active: false,
            alternate_active: false,
            mouse_active: false,
            cursor_restore_pending: false,
        };
        if let Err(error) = session.acquire() {
            let _ = session.restore();
            return Err(error);
        }
        Ok(session)
    }

    fn acquire(&mut self) -> io::Result<()> {
        if self.raw_active && self.alternate_active && self.mouse_active {
            return Ok(());
        }
        debug_assert!(!self.raw_active && !self.alternate_active && !self.mouse_active);

        self.ops.enable_raw()?;
        self.raw_active = true;
        self.cursor_restore_pending = true;

        self.ops.enter_alternate()?;
        self.alternate_active = true;

        self.ops.enable_mouse()?;
        self.mouse_active = true;
        Ok(())
    }

    pub(crate) fn suspend(&mut self) -> io::Result<()> {
        self.release()
    }

    pub(crate) fn resume(&mut self) -> io::Result<()> {
        if self.raw_active && self.alternate_active && self.mouse_active {
            return Ok(());
        }
        if self.raw_active || self.alternate_active || self.mouse_active {
            self.restore()?;
        }
        if let Err(error) = self.acquire() {
            let _ = self.restore();
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn restore(&mut self) -> io::Result<()> {
        self.release()
    }

    fn release(&mut self) -> io::Result<()> {
        let mut first_error = None;

        // The observable postcondition belongs to the main screen: after ARX leaves
        // alternate screen, terminal mouse reporting must be disabled there. A real
        // terminal reproduced leaked reporting when this order was reversed.
        if self.alternate_active {
            match self.ops.leave_alternate() {
                Ok(()) => self.alternate_active = false,
                Err(error) => remember_error(&mut first_error, error),
            }
        }
        if self.mouse_active {
            match self.ops.disable_mouse() {
                Ok(()) => self.mouse_active = false,
                Err(error) => remember_error(&mut first_error, error),
            }
        }
        if self.raw_active {
            match self.ops.disable_raw() {
                Ok(()) => self.raw_active = false,
                Err(error) => remember_error(&mut first_error, error),
            }
        }
        if self.cursor_restore_pending {
            match self.ops.show_cursor() {
                Ok(()) => self.cursor_restore_pending = false,
                Err(error) => remember_error(&mut first_error, error),
            }
        }

        finish_cleanup(first_error)
    }

    pub(crate) async fn suspend_while<F, Fut, T>(&mut self, operation: F) -> io::Result<T>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        self.suspend()?;
        let output = operation().await;
        self.resume()?;
        Ok(output)
    }
}

impl<O: TerminalOps> Drop for TuiTerminalSession<O> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn remember_error(first_error: &mut Option<io::Error>, error: io::Error) {
    if first_error.is_none() {
        *first_error = Some(error);
    }
}

fn finish_cleanup(first_error: Option<io::Error>) -> io::Result<()> {
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    use super::*;

    const ENABLE_RAW: &str = "enable_raw";
    const DISABLE_RAW: &str = "disable_raw";
    const ENTER_ALTERNATE: &str = "enter_alternate";
    const LEAVE_ALTERNATE: &str = "leave_alternate";
    const ENABLE_MOUSE: &str = "enable_mouse";
    const DISABLE_MOUSE: &str = "disable_mouse";
    const SHOW_CURSOR: &str = "show_cursor";

    #[derive(Debug, Default)]
    struct MockState {
        calls: Vec<&'static str>,
        failures: BTreeSet<&'static str>,
    }

    #[derive(Clone, Debug, Default)]
    struct MockHandle(Arc<Mutex<MockState>>);

    impl MockHandle {
        fn calls(&self) -> Vec<&'static str> {
            self.0.lock().unwrap().calls.clone()
        }

        fn clear_calls(&self) {
            self.0.lock().unwrap().calls.clear();
        }

        fn fail(&self, operation: &'static str) {
            self.0.lock().unwrap().failures.insert(operation);
        }

        fn allow(&self, operation: &'static str) {
            self.0.lock().unwrap().failures.remove(operation);
        }
    }

    #[derive(Debug)]
    struct MockOps {
        handle: MockHandle,
    }

    impl MockOps {
        fn new(handle: MockHandle) -> Self {
            Self { handle }
        }

        fn call(&mut self, operation: &'static str) -> io::Result<()> {
            let mut state = self.handle.0.lock().unwrap();
            state.calls.push(operation);
            if state.failures.contains(operation) {
                Err(io::Error::other(format!("{operation} failed")))
            } else {
                Ok(())
            }
        }
    }

    impl TerminalOps for MockOps {
        fn enable_raw(&mut self) -> io::Result<()> {
            self.call(ENABLE_RAW)
        }
        fn disable_raw(&mut self) -> io::Result<()> {
            self.call(DISABLE_RAW)
        }
        fn enter_alternate(&mut self) -> io::Result<()> {
            self.call(ENTER_ALTERNATE)
        }
        fn leave_alternate(&mut self) -> io::Result<()> {
            self.call(LEAVE_ALTERNATE)
        }
        fn enable_mouse(&mut self) -> io::Result<()> {
            self.call(ENABLE_MOUSE)
        }
        fn disable_mouse(&mut self) -> io::Result<()> {
            self.call(DISABLE_MOUSE)
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            self.call(SHOW_CURSOR)
        }
    }

    fn entered() -> (TuiTerminalSession<MockOps>, MockHandle) {
        let handle = MockHandle::default();
        let session = TuiTerminalSession::enter_with(MockOps::new(handle.clone())).unwrap();
        (session, handle)
    }

    #[test]
    fn enter_acquires_raw_alternate_and_mouse_in_order() {
        let (session, handle) = entered();
        assert!(session.raw_active);
        assert!(session.alternate_active);
        assert!(session.mouse_active);
        assert_eq!(
            handle.calls(),
            vec![ENABLE_RAW, ENTER_ALTERNATE, ENABLE_MOUSE]
        );
    }

    #[test]
    fn restore_disables_mouse_after_returning_to_main_screen() {
        let (mut session, handle) = entered();
        handle.clear_calls();
        session.restore().unwrap();
        assert_eq!(
            handle.calls(),
            vec![LEAVE_ALTERNATE, DISABLE_MOUSE, DISABLE_RAW, SHOW_CURSOR]
        );
        assert!(!session.raw_active);
        assert!(!session.alternate_active);
        assert!(!session.mouse_active);
        assert!(!session.cursor_restore_pending);
    }

    #[test]
    fn restore_is_idempotent() {
        let (mut session, handle) = entered();
        session.restore().unwrap();
        let after_first = handle.calls();
        session.restore().unwrap();
        assert_eq!(handle.calls(), after_first);
    }

    #[test]
    fn partial_enter_failure_rolls_back_completed_stages() {
        let handle = MockHandle::default();
        handle.fail(ENABLE_MOUSE);
        let result = TuiTerminalSession::enter_with(MockOps::new(handle.clone()));
        assert!(result.is_err());
        assert_eq!(
            handle.calls(),
            vec![
                ENABLE_RAW,
                ENTER_ALTERNATE,
                ENABLE_MOUSE,
                LEAVE_ALTERNATE,
                DISABLE_RAW,
                SHOW_CURSOR,
            ]
        );
    }

    #[test]
    fn cleanup_failure_does_not_skip_later_operations() {
        let (mut session, handle) = entered();
        handle.clear_calls();
        handle.fail(DISABLE_MOUSE);
        let result = session.restore();
        assert!(result.is_err());
        assert_eq!(
            handle.calls(),
            vec![LEAVE_ALTERNATE, DISABLE_MOUSE, DISABLE_RAW, SHOW_CURSOR]
        );
        assert!(session.mouse_active);
        assert!(!session.alternate_active);
        assert!(!session.raw_active);

        handle.allow(DISABLE_MOUSE);
        handle.clear_calls();
        session.restore().unwrap();
        assert_eq!(handle.calls(), vec![DISABLE_MOUSE]);
    }

    #[test]
    fn drop_best_effort_restores_without_panicking() {
        let handle = MockHandle::default();
        {
            let _session = TuiTerminalSession::enter_with(MockOps::new(handle.clone())).unwrap();
            handle.clear_calls();
        }
        assert_eq!(
            handle.calls(),
            vec![LEAVE_ALTERNATE, DISABLE_MOUSE, DISABLE_RAW, SHOW_CURSOR]
        );
    }

    #[test]
    fn suspend_disables_mouse_after_returning_to_main_screen() {
        let (mut session, handle) = entered();
        handle.clear_calls();
        session.suspend().unwrap();
        assert_eq!(
            handle.calls(),
            vec![LEAVE_ALTERNATE, DISABLE_MOUSE, DISABLE_RAW, SHOW_CURSOR]
        );
        assert!(!session.raw_active);
        assert!(!session.alternate_active);
        assert!(!session.mouse_active);

        handle.clear_calls();
        session.resume().unwrap();
        assert_eq!(
            handle.calls(),
            vec![ENABLE_RAW, ENTER_ALTERNATE, ENABLE_MOUSE]
        );
    }

    #[tokio::test]
    async fn suspended_operation_resumes_even_when_external_operation_fails() {
        let (mut session, handle) = entered();
        handle.clear_calls();
        let external = session
            .suspend_while(|| async { Err::<(), _>(io::Error::other("external failed")) })
            .await
            .unwrap();
        assert!(external.is_err());
        assert_eq!(
            handle.calls(),
            vec![
                LEAVE_ALTERNATE,
                DISABLE_MOUSE,
                DISABLE_RAW,
                SHOW_CURSOR,
                ENABLE_RAW,
                ENTER_ALTERNATE,
                ENABLE_MOUSE,
            ]
        );
    }

    #[test]
    fn resume_failure_rolls_back_partial_reacquire() {
        let (mut session, handle) = entered();
        session.suspend().unwrap();
        handle.clear_calls();
        handle.fail(ENABLE_MOUSE);
        let result = session.resume();
        assert!(result.is_err());
        assert_eq!(
            handle.calls(),
            vec![
                ENABLE_RAW,
                ENTER_ALTERNATE,
                ENABLE_MOUSE,
                LEAVE_ALTERNATE,
                DISABLE_RAW,
                SHOW_CURSOR,
            ]
        );
    }
}
