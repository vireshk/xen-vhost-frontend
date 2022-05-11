// Copyright 2022 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use field_offset::offset_of;
use std::os::raw::{c_int, c_void};
use std::ptr;
use std::slice;

use super::{Error, Result};
use libxen_sys::{
    domid_t, ioreq, ioservid_t, shared_iopage, xen_pfn_t, XENMEM_resource_ioreq_server,
    XC_PAGE_SHIFT,
};
use xen_ioctls::{
    xenforeignmemory_map, xenforeignmemory_map_resource, xenforeignmemory_unmap,
    xenforeignmemory_unmap_resource, XenForeignMemoryResourceHandle,
};

pub struct XenForeignMemory {
    res: Option<XenForeignMemoryResourceHandle>,
    ioreq: *mut ioreq,
    addr: Vec<(*mut c_void, u64)>,
}

impl XenForeignMemory {
    pub fn new() -> Result<Self> {
        Ok(Self {
            res: None,
            ioreq: ptr::null_mut::<ioreq>(),
            addr: Vec::new(),
        })
    }

    pub fn map_resource(&mut self, domid: domid_t, id: ioservid_t) -> Result<()> {
        let paddr = ptr::null_mut::<c_void>();
        let resource_handle = xenforeignmemory_map_resource(
            domid,
            XENMEM_resource_ioreq_server,
            id as u32,
            1,
            1,
            paddr,
            libc::PROT_READ | libc::PROT_WRITE,
            0,
        )
        .map_err(Error::XenIoctlError)?;

        let offset = offset_of!(shared_iopage => vcpu_ioreq).get_byte_offset();
        self.ioreq = unsafe { resource_handle.addr.add(offset) } as *mut ioreq;
        self.res = Some(resource_handle);
        Ok(())
    }

    fn unmap_resource(&mut self) -> Result<()> {
        if let Some(res) = &self.res {
            xenforeignmemory_unmap_resource(res).map_err(Error::XenIoctlError)?;
            self.res = None;
        }

        Ok(())
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
        for (i, pfn) in pfn.iter_mut().enumerate().take(num as usize) {
            *pfn = base + i as u64;
        }

        let addr = xenforeignmemory_map(
            domid,
            libc::PROT_READ | libc::PROT_WRITE,
            num,
            pfn.as_ptr(),
            ptr::null_mut::<c_int>(),
        )
        .map_err(Error::XenIoctlError)?;

        self.addr.push((addr, num));
        Ok(addr)
    }

    pub fn unmap_mem(&mut self) -> Result<()> {
        for (addr, n) in &self.addr {
            if xenforeignmemory_unmap(*addr, *n).is_err() {
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
    }
}
