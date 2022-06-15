// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use field_offset::offset_of;
use std::os::raw::c_void;
use std::ptr;
use std::slice;

use super::{Error, Result};
use xen_bindings::bindings::{ioreq, ioservid_t, shared_iopage, XENMEM_resource_ioreq_server};
use xen_ioctls::{
    xenforeignmemory_map_resource, xenforeignmemory_unmap_resource, XenForeignMemoryResourceHandle,
};

pub struct XenForeignMemory {
    res: Option<XenForeignMemoryResourceHandle>,
    ioreq: *mut ioreq,
}

impl XenForeignMemory {
    pub fn new() -> Result<Self> {
        Ok(Self {
            res: None,
            ioreq: ptr::null_mut::<ioreq>(),
        })
    }

    pub fn map_resource(&mut self, domid: u16, id: ioservid_t) -> Result<()> {
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

        // SAFETY: Safe as offset is within range.
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
        // SAFETY: Safe as offset is within range.
        unsafe { self.ioreq.offset(vcpu as isize) }
    }

    pub fn ioreq(&self, vcpu: u32) -> Result<&mut ioreq> {
        let ioreq = self.ioreq_offset(vcpu);

        // SAFETY: Safe as we slice is guaranteed to be valid.
        Ok(unsafe { &mut slice::from_raw_parts_mut(ioreq, 1)[0] })
    }
}

impl Drop for XenForeignMemory {
    fn drop(&mut self) {
        self.unmap_resource().unwrap();
    }
}
