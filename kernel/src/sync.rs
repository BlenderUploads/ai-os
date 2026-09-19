//! Minimal synchronisation primitives.
//!
//! HALCYON is single-CPU, so contention only ever comes from an interrupt
//! handler pre-empting a lock holder. `SpinLock` therefore masks interrupts
//! for the duration of the critical section, which makes it safe to take the
//! same lock from both thread and interrupt context.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::port;

pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> SpinGuard<'_, T> {
        let had_interrupts = port::interrupts_enabled();
        port::cli();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinGuard {
            lock: self,
            restore_interrupts: had_interrupts,
        }
    }

    /// Take the lock if it is free. Used by the panic path, which must never
    /// deadlock on a lock the faulting code was already holding.
    pub fn try_lock(&self) -> Option<SpinGuard<'_, T>> {
        let had_interrupts = port::interrupts_enabled();
        port::cli();
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(SpinGuard {
                lock: self,
                restore_interrupts: had_interrupts,
            })
        } else {
            if had_interrupts {
                port::sti();
            }
            None
        }
    }

    /// Break the lock unconditionally. Only the panic handler may do this.
    pub unsafe fn force_unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }

    /// Reach the contents while holding no lock at all. Only for the panic
    /// screen, after `force_unlock`.
    pub unsafe fn get_unchecked(&self) -> &mut T {
        &mut *self.value.get()
    }
}

pub struct SpinGuard<'a, T> {
    lock: &'a SpinLock<T>,
    restore_interrupts: bool,
}

impl<T> Deref for SpinGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        if self.restore_interrupts {
            port::sti();
        }
    }
}

/// A value written once during boot and read freely thereafter.
pub struct Once<T> {
    initialised: AtomicBool,
    value: UnsafeCell<Option<T>>,
}

unsafe impl<T: Send + Sync> Sync for Once<T> {}

impl<T> Once<T> {
    pub const fn new() -> Self {
        Self {
            initialised: AtomicBool::new(false),
            value: UnsafeCell::new(None),
        }
    }

    pub fn set(&self, value: T) {
        unsafe { *self.value.get() = Some(value) };
        self.initialised.store(true, Ordering::Release);
    }

    pub fn get(&self) -> Option<&T> {
        if self.initialised.load(Ordering::Acquire) {
            unsafe { (*self.value.get()).as_ref() }
        } else {
            None
        }
    }
}
