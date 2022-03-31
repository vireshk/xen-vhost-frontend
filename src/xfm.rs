// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::slice;
use std::os::raw::{c_int, c_void};
use std::ptr;

use field_offset::offset_of;

use libxen_sys::*;
use super::{Error, Result};

pub struct XenForeignMemory {
    xfh: *mut xenforeignmemory_handle,
    res: Option<*mut xenforeignmemory_resource_handle>,
    ioreq: *mut ioreq,
    addr: Vec<(*mut c_void, u64)>,
}

impl XenForeignMemory {
    pub fn new() -> Result<Self> {
        let xfh = unsafe {
            xenforeignmemory_open(ptr::null_mut::<xentoollog_logger>(), 0)
        };

        if xfh.is_null() {
            return Err(Error::XsError);
        }

        Ok (Self {
            xfh,
            res: None,
            ioreq: ptr::null_mut::<ioreq>(),
            addr: Vec::new(),
        })
    }

    pub fn map_resource(&mut self, domid: domid_t, id: ioservid_t) -> Result<()> {
        let mut paddr = ptr::null_mut::<c_void>();
        let res = unsafe {
            xenforeignmemory_map_resource(
                self.xfh, domid, XENMEM_resource_ioreq_server, id as u32,
                1, 1,
                ptr::addr_of_mut!(paddr),
                libc::PROT_READ | libc::PROT_WRITE, 0,
            )
        };

        if res.is_null() {
            Err(Error::XsError)
        } else {
            let offset  = offset_of!(shared_iopage => vcpu_ioreq).get_byte_offset();
            self.ioreq = unsafe { paddr.offset(offset as isize) } as *mut ioreq;
            self.res = Some(res);
            Ok(())
        }
    }

    fn unmap_resource(&mut self) -> Result<()> {
        if self.res.is_none() {
            return Ok(());
        }

        let ret = unsafe { xenforeignmemory_unmap_resource(self.xfh, self.res.unwrap()) };
        if ret < 0 {
            Err(Error::XsError)
        } else {
            self.res = None;
            Ok(())
        }
    }

    fn ioreq_offset(&self, vcpu: u32) -> *mut ioreq {
        unsafe { self.ioreq.offset(vcpu as isize) }
    }

    pub fn ioreq(&self, vcpu: u32) -> Result<&mut ioreq> {
        let ioreq = self.ioreq_offset(vcpu);

        Ok(unsafe { &mut slice::from_raw_parts_mut(ioreq, 1)[0] })
    }

    pub fn map_mem(&mut self, domid: domid_t, paddr: u64, size: u64) -> Result<*mut c_void> {
        let base = paddr >> XC_PAGE_SHIFT;
        let mut num = size >> XC_PAGE_SHIFT;
        if num << XC_PAGE_SHIFT != size {
            num += 1;
        }

        let mut pfn: Vec<xen_pfn_t> = vec![0; num as usize];
        for i in 0..num as usize {
            pfn[i] = base + i as u64;
        }

        let addr = unsafe { xenforeignmemory_map(self.xfh, domid as u32, libc::PROT_READ | libc::PROT_WRITE, num, pfn.as_ptr(), ptr::null_mut::<c_int>()) };
        if addr.is_null() {
            Err(Error::XsError)
        } else {
            self.addr.push((addr, num));

            Ok(addr)
        }
    }

    pub fn unmap_mem(&mut self) -> Result<()> {
        for (addr, n) in &self.addr {
            let ret = unsafe { xenforeignmemory_unmap(self.xfh, *addr, *n) };
            if ret < 0 {
                println!("XenForeignMemory: failed to unmap: {:?}", (*addr, *n));
            }
        }

        Ok(())
    }
}

impl Drop for XenForeignMemory {
    fn drop(&mut self) {
        self.unmap_mem().unwrap();
        self.unmap_resource().unwrap();
        unsafe{ xenforeignmemory_close(self.xfh); }
    }
}
