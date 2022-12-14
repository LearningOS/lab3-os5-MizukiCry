//! Implementation of [`Processor`] and Intersection of control flow
//!
//! Here, the continuous operation of user apps in CPU is maintained,
//! the current running state of CPU is recorded,
//! and the replacement and transfer of control flow of different applications are executed.


use super::__switch;
use super::{fetch_task, TaskStatus};
use super::{TaskContext, TaskControlBlock};
use crate::config::MAX_SYSCALL_NUM;
use crate::mm::{VirtAddr, MapPermission, VPNRange};
use crate::sync::UPSafeCell;
use crate::timer::get_time_ms;
use crate::trap::TrapContext;
use alloc::sync::Arc;
use lazy_static::*;

/// Processor management structure
pub struct Processor {
    /// The task currently executing on the current processor
    current: Option<Arc<TaskControlBlock>>,
    /// The basic control flow of each core, helping to select and switch process
    idle_task_cx: TaskContext,
}

impl Processor {
    pub fn new() -> Self {
        Self {
            current: None,
            idle_task_cx: TaskContext::zero_init(),
        }
    }
    fn get_idle_task_cx_ptr(&mut self) -> *mut TaskContext {
        &mut self.idle_task_cx as *mut _
    }
    pub fn take_current(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.current.take()
    }
    pub fn current(&self) -> Option<Arc<TaskControlBlock>> {
        self.current.as_ref().map(|task| Arc::clone(task))
    }

    fn task_mmap(&self, start: usize, len: usize, port: usize) -> isize {
        let start_va = VirtAddr::from(start);
        let end_va = VirtAddr::from(start + len);
        if !start_va.aligned() || (port & !0x7) != 0 || (port & 0x7) == 0 {
            return -1;
        }
        let current_task = self.current().unwrap();
        let memory_set = &mut current_task.inner_exclusive_access().memory_set;
        let start_vpn = start_va.floor();
        let end_vpn = end_va.ceil();
        for vpn in VPNRange::new(start_vpn, end_vpn) {
            if let Some(pte) = memory_set.translate(vpn) {
                if pte.is_valid() {
                    return -1;
                }
            }
        }
        let map_perm = MapPermission::from_bits((port as u8) << 1).unwrap() | MapPermission::U;
        memory_set.insert_framed_area(start_va, end_va, map_perm);
        0
    }

    fn task_munmap(&self, start: usize, len: usize) -> isize {
        let start_va = VirtAddr::from(start);
        let end_va = VirtAddr::from(start + len);
        if !start_va.aligned() {
            return -1;
        }
        let current_task = self.current().unwrap();
        let memory_set = &mut current_task.inner_exclusive_access().memory_set;
        let start_vpn = start_va.floor();
        let end_vpn = end_va.ceil();
        for vpn in VPNRange::new(start_vpn, end_vpn) {
            if let Some(pte) = memory_set.translate(vpn) {
                if !pte.is_valid() {
                    return -1;
                }
            } else {
                return -1;
            }
        }
        memory_set.unmap(start_vpn, end_vpn);
        0
    }

    fn count_syscall(&self, syscall_id: usize) {
        if syscall_id < MAX_SYSCALL_NUM {
            self.current().unwrap().inner_exclusive_access().syscall_times[syscall_id] += 1;
        }
    }

    fn current_task_status(&self) -> TaskStatus {
        self.current().unwrap().inner_exclusive_access().task_status
    }

    fn current_syscall_times(&self) -> [u32; MAX_SYSCALL_NUM] {
        *self.current().unwrap().inner_exclusive_access().syscall_times
    }

    fn current_run_time(&self) -> usize {
        get_time_ms() - self.current().unwrap().inner_exclusive_access().start_time.unwrap()
    }
}

lazy_static! {
    /// PROCESSOR instance through lazy_static!
    pub static ref PROCESSOR: UPSafeCell<Processor> = unsafe { UPSafeCell::new(Processor::new()) };
}

/// The main part of process execution and scheduling
///
/// Loop fetch_task to get the process that needs to run,
/// and switch the process through __switch
pub fn run_tasks() {
    loop {
        let mut processor = PROCESSOR.exclusive_access();
        if let Some(task) = fetch_task() {
            let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
            // access coming task TCB exclusively
            let mut task_inner = task.inner_exclusive_access();
            let next_task_cx_ptr = &task_inner.task_cx as *const TaskContext;
            if task_inner.start_time.is_none() {
                task_inner.start_time = Some(get_time_ms());
            }
            task_inner.task_status = TaskStatus::Running;
            drop(task_inner);
            // release coming task TCB manually
            processor.current = Some(task);
            // release processor manually
            drop(processor);
            unsafe {
                __switch(idle_task_cx_ptr, next_task_cx_ptr);
            }
        }
    }
}

/// Get current task through take, leaving a None in its place
pub fn take_current_task() -> Option<Arc<TaskControlBlock>> {
    PROCESSOR.exclusive_access().take_current()
}

/// Get a copy of the current task
pub fn current_task() -> Option<Arc<TaskControlBlock>> {
    PROCESSOR.exclusive_access().current()
}

/// Get token of the address space of current task
pub fn current_user_token() -> usize {
    let task = current_task().unwrap();
    let token = task.inner_exclusive_access().get_user_token();
    token
}

/// Get the mutable reference to trap context of current task
pub fn current_trap_cx() -> &'static mut TrapContext {
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .get_trap_cx()
}

/// Return to idle control flow for new scheduling
pub fn schedule(switched_task_cx_ptr: *mut TaskContext) {
    let mut processor = PROCESSOR.exclusive_access();
    let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
    drop(processor);
    unsafe {
        __switch(switched_task_cx_ptr, idle_task_cx_ptr);
    }
}

pub fn task_mmap(start: usize, len: usize, port: usize) -> isize {
    PROCESSOR.exclusive_access().task_mmap(start, len, port)
}

pub fn task_munmap(start: usize, len: usize) -> isize {
    PROCESSOR.exclusive_access().task_munmap(start, len)
}

pub fn count_syscall(syscall_id: usize) {
    PROCESSOR.exclusive_access().count_syscall(syscall_id);
}

pub fn current_task_status() -> TaskStatus {
    PROCESSOR.exclusive_access().current_task_status()
}

pub fn current_syscall_times() -> [u32; MAX_SYSCALL_NUM] {
    PROCESSOR.exclusive_access().current_syscall_times()
}

pub fn current_run_time() -> usize {
    PROCESSOR.exclusive_access().current_run_time()
}