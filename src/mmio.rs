// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    mem,
    sync::Arc,
    sync::Mutex,
    thread::{Builder, JoinHandle},
};

use vhost::vhost_user::message::VhostUserProtocolFeatures;
use vhost_user_frontend::{Generic, VirtioDevice};
use vhost_user_frontend::{GuestMemoryMmap, GuestRegionMmap};
use virtio_bindings::virtio_config::{VIRTIO_F_IOMMU_PLATFORM, VIRTIO_F_VERSION_1};
use virtio_bindings::virtio_ring::{__virtio16, vring_avail, vring_used, vring_used_elem};
use virtio_queue::{Descriptor, Queue, QueueT};
use vm_memory::ByteValued;
use vm_memory::{
    guest_memory::FileOffset, GuestAddress, GuestMemoryAtomic, GuestMemoryRegion, MmapRange,
    MmapRegion, MmapXenFlags,
};

use vmm_sys_util::eventfd::{EventFd, EFD_NONBLOCK};

use super::{device::XenDevice, guest::XenGuest, Error, Result};
use xen_bindings::bindings::{ioreq, IOREQ_READ, IOREQ_WRITE, XC_PAGE_SHIFT, XC_PAGE_SIZE};
use xen_ioctls::xc_domain_info;

// Bus messages
//const VIRTIO_MSG_FFA_ERROR: u8 = 0x00;
const VIRTIO_MSG_FFA_ACTIVATE: u8 = 0x01;
const VIRTIO_MSG_FFA_DEACTIVATE: u8 = 0x02;
const VIRTIO_MSG_FFA_CONFIGURE: u8 = 0x03;
//const VIRTIO_MSG_FFA_AREA_SHARE: u8 = 0x04;
//const VIRTIO_MSG_FFA_AREA_UNSHARE: u8 = 0x05;

const VIRTIO_MSG_FFA_VERSION_1_0: u32 = 0x1;
const VIRTIO_MSG_FFA_FEATURE_INDIRECT_MSG_SUPP: u64 = 0x1;
const VIRTIO_MSG_FFA_FEATURE_DIRECT_MSG_SUPP: u64 = 0x2;

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct BusActivate {
	driver_version: u32,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct BusActivateResp {
	device_version: u32,
	features: u64,
	num: u64,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct BusConfigure {
	features: u64,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct BusConfigureResp {
	features: u64,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct BusAreaShare {
	area_id: u32,
	mem_handle: u64,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct BusAreaShareResp {
	area_id: u32,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct BusAreaUnshare {
	area_id: u32,
	mem_handle: u64,
}

// Virtio messages
// const VIRTIO_MSG_CONNECT: u8 = 0x01;
// const VIRTIO_MSG_DISCONNECT: u8 = 0x02;
const VIRTIO_MSG_DEVICE_INFO: u8 = 0x03;
const VIRTIO_MSG_GET_FEATURES: u8 = 0x04;
const VIRTIO_MSG_SET_FEATURES: u8 = 0x05;
const VIRTIO_MSG_GET_CONFIG: u8 = 0x06;
const VIRTIO_MSG_SET_CONFIG: u8 = 0x07;
const VIRTIO_MSG_GET_CONFIG_GEN: u8 = 0x08;
const VIRTIO_MSG_GET_DEVICE_STATUS: u8 = 0x09;
const VIRTIO_MSG_SET_DEVICE_STATUS: u8 = 0x0a;
const VIRTIO_MSG_GET_VQUEUE: u8 = 0x0b;
const VIRTIO_MSG_SET_VQUEUE: u8 = 0x0c;
const VIRTIO_MSG_RESET_VQUEUE: u8 = 0x0d;
// const VIRTIO_MSG_EVENT_CONFIG: u8 = 0x10;
const VIRTIO_MSG_EVENT_AVAIL: u8 = 0x11;
const VIRTIO_MSG_EVENT_USED: u8 = 0x12;

const VIRTIO_MSG_TYPE_RESPONSE: u8 = 0x1;
const VIRTIO_MSG_TYPE_VIRTIO: u8 = 0x0;
const VIRTIO_MSG_TYPE_BUS: u8 = 0x2;

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct GetDeviceInfoResp {
    device_version: u32,
    device_id: u32,
    vendor_id: u32,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct GetFeatures {
    index: u32,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct GetFeaturesResp {
    index: u32,
    features: [u64; 4],
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct SetFeatures {
    index: u32,
    features: [u64; 4],
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct SetFeaturesResp {
    index: u32,
    features: [u64; 4],
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct GetConfig {
    offset: [u8; 3],
    size: u8,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct GetConfigResp {
    offset: [u8; 3],
    size: u8,
    data: [u64; 4],
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct SetConfig {
    offset: [u8; 3],
    size: u8,
    data: [u64; 4],
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct SetConfigResp {
    offset: [u8; 3],
    size: u8,
    data: [u64; 4],
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct GetConfigGenResp {
    generation: u32,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct GetDeviceStatusResp {
    status: u32,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct SetDeviceStatus {
    status: u32,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct GetVqueue {
    index: u32,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct GetVqueueResp {
    index: u32,
    max_size: u32,
    size: u32,
    descriptor_addr: u64,
    driver_addr: u64,
    device_addr: u64,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct SetVqueue {
    index: u32,
    unused: u32,
    size: u32,
    descriptor_addr: u64,
    driver_addr: u64,
    device_addr: u64,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct SetVqueueResp {
    index: u32,
    unused: u32,
    size: u32,
    descriptor_addr: u64,
    driver_addr: u64,
    device_addr: u64,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct ResetVqueue {
    index: u32,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct EventConfig {
    status: u32,
    offset: [u8; 3],
    size: u8,
    value: [u32; 4],
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct EventAvail {
    index: u32,
    next_offset: u32,
    next_wrap: u32,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct EventUsed {
    index: u32,
}

#[derive(Copy, Clone)]
#[repr(C, packed)]
union ReqRespTypes {
    payload: [u8; 36],

    // Bus messages
    bus_activate: BusActivate,
    bus_activate_resp: BusActivateResp,
    bus_configure: BusConfigure,
    bus_configure_resp: BusConfigureResp,
    bus_area_share: BusAreaShare,
    bus_area_share_resp: BusAreaShareResp,
    bus_area_unshare: BusAreaUnshare,

    // virtio messages
    get_device_info_resp: GetDeviceInfoResp,
    get_features: GetFeatures,
    get_features_resp: GetFeaturesResp,
    set_features: SetFeatures,
    set_features_resp: SetFeaturesResp,
    get_config: GetConfig,
    get_config_resp: GetConfigResp,
    set_config: SetConfig,
    set_config_resp: SetConfigResp,
    get_config_gen_resp: GetConfigGenResp,
    get_device_status_resp: GetDeviceStatusResp,
    set_device_status: SetDeviceStatus,
    get_vqueue: GetVqueue,
    get_vqueue_resp: GetVqueueResp,
    set_vqueue: SetVqueue,
    reset_vqueue: ResetVqueue,
    event_config: EventConfig,
    event_avail: EventAvail,
    event_used: EventUsed,
}

impl Default for ReqRespTypes {
    fn default() -> Self {
        Self { payload: [0; 36] }
    }
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct VirtioMsg {
    _type: u8,
    id: u8,
    dev_id: [u8; 2],
    r: ReqRespTypes,
}

const GUEST_RAM0_BASE: u64 = 0x40000000; // 3GB of low RAM @ 1GB
const XEN_GRANT_ADDR_OFF: u64 = 1 << 63;

fn get_dom_size(domid: u16) -> Result<usize> {
    let info = xc_domain_info(domid, 1);

    if info.len() != 1 {
        Err(Error::InvalidDomainInfo(info.len(), domid, 0))
    } else if info[0].domid != domid {
        Err(Error::InvalidDomainInfo(
            info.len(),
            domid,
            info[0].domid as usize,
        ))
    } else {
        Ok((info[0].nr_pages as usize - 4) << XC_PAGE_SHIFT)
    }
}

struct VirtQueue {
    ready: u32,
    size: u32,
    size_max: u32,
    desc: u64,
    avail: u64,
    used: u64,

    // Guest to device
    kick: EventFd,
}

pub struct XenMmio {
    addr: u64,
    version: u8,
    vendor_id: u32,
    status: u32,
    queues_count: usize,
    queues: Vec<(usize, Queue, EventFd)>,
    vq: Vec<VirtQueue>,
    regions: Vec<GuestRegionMmap>,
    foreign_mapping: bool,
    guest_size: usize,
    guest: Arc<XenGuest>,
    request: VirtioMsg,
    response: VirtioMsg,
    respond: bool,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl XenMmio {
    pub fn new(
        gdev: &Generic,
        guest: Arc<XenGuest>,
        addr: u64,
        foreign_mapping: bool,
    ) -> Result<Self> {
        let sizes = gdev.queue_max_sizes();
        let guest_size = get_dom_size(guest.fe_domid)?;

        let mut mmio = Self {
            addr,
            version: 2,
            vendor_id: 0x4d564b4c,
            status: 0,
            queues_count: sizes.len(),
            queues: Vec::with_capacity(sizes.len()),
            vq: Vec::new(),
            regions: Vec::new(),
            foreign_mapping,
            guest_size,
            guest: guest.clone(),
            request: VirtioMsg::default(),
            response: VirtioMsg::default(),
            respond: false,
            handle: Mutex::new(None),
        };

        let xfm = guest.xfm.lock().unwrap();
        let ioreq = xfm.ioreq(0).unwrap();
        let xec = guest.xec.lock().unwrap();

        for (index, size) in sizes.iter().enumerate() {
            let kick = EventFd::new(EFD_NONBLOCK).unwrap();

            guest
                .xdm
                .lock()
                .unwrap()
                .set_ioeventfd(&kick, ioreq, xec.ports(), addr, index as u32, true)
                .unwrap();

            mmio.vq.push(VirtQueue {
                ready: 0,
                size: 0,
                size_max: *size as u32,
                desc: 0,
                avail: 0,
                used: 0,
                kick,
            });
        }

        // Foreign memory must be mapped in advance as it takes considerable amount of time to do
        // it, and doing it later times out the guest kernel.
        if foreign_mapping {
            mmio.map_foreign_region(guest.fe_domid)?;
        }

        Ok(mmio)
    }

    pub(crate) fn setup_vmsg_events(&mut self, dev: Arc<XenDevice>) -> Result<()> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/virtio-msg-0")
            .map_err(|_| Error::VirtioLegacyNotSupported)?;

        let request = unsafe {
            // Cast the struct to a mutable byte slice
            std::slice::from_raw_parts_mut(
                &mut self.request as *mut VirtioMsg as *mut u8,
                mem::size_of::<VirtioMsg>(),
            )
        };

        let response = unsafe {
            // Cast the struct to a mutable byte slice
            std::slice::from_raw_parts_mut(
                &mut self.response as *mut VirtioMsg as *mut u8,
                mem::size_of::<VirtioMsg>(),
            )
        };

        *self.handle.lock().unwrap() = Some(
            Builder::new()
                .spawn(move || {
                    while file.read_exact(request).is_ok() {
                        let mut mmio = dev.mmio.lock().unwrap();

                        mmio.handle_virtio_messages(&dev).unwrap();
                        file.write_all(response).unwrap();
                    }
                })
                .unwrap(),
        );

        Ok(())
    }

    pub(crate) fn send_event_used(&self, file: &mut File) {
        let mut request: VirtioMsg = VirtioMsg::default();

        request._type = VIRTIO_MSG_TYPE_VIRTIO;
        request.id = VIRTIO_MSG_EVENT_USED;

        let buf = unsafe {
            // Cast the struct to a mutable byte slice
            std::slice::from_raw_parts_mut(
                &mut request as *mut VirtioMsg as *mut u8,
                mem::size_of::<VirtioMsg>(),
            )
        };

        // With the rust-vmm setup, we don't get the virtqueue number here. Send the event for each
        // virtqueue to make it work.
        for i in 0..self.queues_count {
            request.r.event_used.index = i as u32;
            file.write_all(buf).unwrap();
        }
    }

    fn config_read(&self, gdev: &Generic, offset: u64, size: u8) -> Result<u64> {
        let mut data: u64 = 0;
        gdev.read_config(offset, &mut data.as_mut_slice()[0..size as usize]);

        Ok(data)
    }

    fn config_write(&self, gdev: &mut Generic, data: u64, offset: u64, size: u8) -> Result<()> {
        gdev.write_config(offset, &data.to_ne_bytes()[0..size as usize]);
        Ok(())
    }

    fn io_read(&mut self, ioreq: &mut ioreq, offset: u64) -> Result<()> {
        let index = (offset / 8) as usize;

        unsafe {
            let ptr = &mut self.response as *mut VirtioMsg as *mut u64;
            ioreq.data = *ptr.add(index);
        }

        Ok(())
    }

    fn handle_virtio_bus_msg(&mut self) -> Result<()> {
        match self.request.id {
            VIRTIO_MSG_FFA_ACTIVATE => {
                self.response.r.bus_activate_resp.device_version = VIRTIO_MSG_FFA_VERSION_1_0;
                self.response.r.bus_activate_resp.features =
                    VIRTIO_MSG_FFA_FEATURE_INDIRECT_MSG_SUPP |
                    VIRTIO_MSG_FFA_FEATURE_DIRECT_MSG_SUPP;
                self.response.r.bus_activate_resp.num = 1;

                self.respond = true;
            }

            VIRTIO_MSG_FFA_DEACTIVATE => {
                self.respond = false;
            }

            VIRTIO_MSG_FFA_CONFIGURE => {
                unsafe { self.response.r.bus_configure_resp.features = self.request.r.bus_configure.features };
                self.respond = true;
            }

            x => println!("handle_virtio_bus_msg() failed, unknown msg id {}", x),
        }

        Ok(())
    }

    fn handle_virtio_msg(&mut self, dev: &XenDevice) -> Result<()> {
        self.response.dev_id = self.request.dev_id;

        match self.request.id {
            VIRTIO_MSG_DEVICE_INFO => {
                let gdev = &mut dev.gdev.lock().unwrap();

                self.response.r.get_device_info_resp.device_version = self.version as u32;
                self.response.r.get_device_info_resp.device_id = gdev.device_type();
                self.response.r.get_device_info_resp.vendor_id = self.vendor_id;
                self.respond = true;
            }

            VIRTIO_MSG_GET_FEATURES => {
                let gdev = &mut dev.gdev.lock().unwrap();

                let mut features = gdev.device_features();
                features |= 1 << VIRTIO_F_VERSION_1;
                features |= 1 << VIRTIO_F_IOMMU_PLATFORM;

                unsafe {
                    self.response.r.get_features_resp.index = self.request.r.get_features.index;
                    self.response.r.get_features_resp.features[0] = features;
                }
                self.respond = true;
            }

            VIRTIO_MSG_SET_FEATURES => {
                let gdev = &mut dev.gdev.lock().unwrap();

                let driver_features = unsafe { self.request.r.set_features.features[0] };

                if (driver_features & (1 << VIRTIO_F_VERSION_1)) == 0 {
                    return Err(Error::VirtioLegacyNotSupported);
                }

                // Lets negotiate features.
                gdev.negotiate_features(driver_features, VhostUserProtocolFeatures::XEN_MMAP)
                    .map_err(Error::VhostFrontendError)?;

                // Linux doesn't use below, still send it.
                //unsafe {
                //    let mut features = gdev.device_features();
                //    features |= 1 << VIRTIO_F_VERSION_1;
                //    features |= 1 << VIRTIO_F_IOMMU_PLATFORM;

                //    self.response.r.set_features_resp.index = self.request.r.set_features.index;
                //    self.response.r.set_features_resp.features[0] = features;
                //}
                self.respond = true;
            }

            VIRTIO_MSG_GET_CONFIG => {
                let gdev = &mut dev.gdev.lock().unwrap();

                let size = unsafe { self.request.r.get_config.size };

                if size == 0 || size > 8 {
                    return Err(Error::InvalidSize(size));
                }

                let data = unsafe { self.request.r.get_config.offset };
                let mut offset = data[2] as u64;
                offset = (offset << 8) | data[1] as u64;
                offset = (offset << 8) | data[0] as u64;

                let data = self.config_read(gdev, offset, size)?;
                unsafe {
                    self.response.r.get_config_resp.data[0] = data;
                    self.response.r.get_config_resp.offset = self.request.r.get_config.offset;
                    self.response.r.get_config_resp.size = self.request.r.get_config.size;
                }
                self.respond = true;
            }

            VIRTIO_MSG_SET_CONFIG => {
                let gdev = &mut dev.gdev.lock().unwrap();

                let size = unsafe { self.request.r.set_config.size };

                if size == 0 || size > 8 {
                    return Err(Error::InvalidSize(size));
                }

                let data = unsafe { self.request.r.set_config.offset };
                let mut offset = data[2] as u64;
                offset = (offset << 8) | data[1] as u64;
                offset = (offset << 8) | data[0] as u64;

                self.config_write(
                    gdev,
                    unsafe { self.request.r.set_config.data[0] },
                    offset,
                    size,
                )?;

                // Linux doesn't use below, still send it.
                unsafe {
                    self.response.r.set_config_resp.offset = self.request.r.set_config.offset;
                    self.response.r.set_config_resp.data = self.request.r.set_config.data;
                    self.response.r.set_config_resp.size = self.request.r.set_config.size;
                }
                self.respond = true;
            }

            VIRTIO_MSG_GET_CONFIG_GEN => {
                self.response.r.get_config_gen_resp.generation = 0;
                self.respond = true;
            }

            VIRTIO_MSG_GET_DEVICE_STATUS => {
                self.response.r.get_device_status_resp.status = self.status;
                self.respond = true;
            }
            VIRTIO_MSG_SET_DEVICE_STATUS => {
                unsafe { self.status = self.request.r.set_device_status.status };
                self.respond = false;
            }

            VIRTIO_MSG_GET_VQUEUE => {
                let index = unsafe { self.request.r.get_vqueue.index };
                let vq = &self.vq[index as usize];

                self.response.r.get_vqueue_resp.index = index;
                self.response.r.get_vqueue_resp.max_size = vq.size_max.into();
                self.respond = true;
            }

            VIRTIO_MSG_SET_VQUEUE => {
                let index = unsafe { self.request.r.set_vqueue.index };
                let vq = &mut self.vq[index as usize];

                vq.size = unsafe { self.request.r.set_vqueue.size as u32 };
                vq.desc = unsafe { self.request.r.set_vqueue.descriptor_addr };
                vq.avail = unsafe { self.request.r.set_vqueue.driver_addr };
                vq.used = unsafe { self.request.r.set_vqueue.device_addr };

                // Initialize the virtqueue
                self.init_vq(dev.guest.fe_domid, index as usize)?;

                // Wait for all virtqueues to get initialized.
                if self.queues.len() == self.queues_count {
                    self.activate_device(dev, dev.guest.fe_domid)?;
                }
                self.respond = true;
            }

            VIRTIO_MSG_RESET_VQUEUE => {
                self.destroy_vq();
                self.respond = false;
            }

            VIRTIO_MSG_EVENT_AVAIL => {
                // This is generally handled in the Linux kernel for MMIO protocol now. But we
                // can't use it. Notify backend.
                let index = unsafe { self.request.r.event_avail.index };
                self.vq[index as usize]
                    .kick
                    .write(1)
                    .map_err(Error::EventFdWriteFailed)?;
                self.respond = false;
            }

            x => println!("handle_virtio_msg() failed, unknown msg id {}", x),
        }

        Ok(())
    }

    fn handle_virtio_messages(&mut self, dev: &XenDevice) -> Result<()> {
        // Erase previous response.
        self.response = VirtioMsg::default();
        self.response._type = self.request._type | VIRTIO_MSG_TYPE_RESPONSE;
        self.response.id = self.request.id;

        match self.request._type {
            VIRTIO_MSG_TYPE_VIRTIO => self.handle_virtio_msg(dev),
            VIRTIO_MSG_TYPE_BUS => self.handle_virtio_bus_msg(),
            _ => Err(Error::InvalidReqType(self.request._type)),
        }
    }

    fn io_write(&mut self, ioreq: &mut ioreq, dev: &XenDevice, offset: u64) -> Result<()> {
        let index = (offset / 8) as usize;

        unsafe {
            let ptr = &mut self.request as *mut VirtioMsg as *mut u64;
            *ptr.add(index) = ioreq.data;
        }

        // Ignore the first four writes, act only after the whole request is written.
        if index != 4 {
            return Ok(());
        }

        self.handle_virtio_messages(dev)
    }

    fn sort_regions(&mut self) {
        self.regions
            .sort_by(|a, b| a.start_addr().partial_cmp(&b.start_addr()).unwrap());
    }

    fn map_region(
        &mut self,
        addr: GuestAddress,
        size: usize,
        path: &str,
        flags: u32,
        data: u32,
    ) -> Result<()> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();

        let range = MmapRange::new(size, Some(FileOffset::new(file, 0)), addr, flags, data);
        let region = GuestRegionMmap::new(MmapRegion::from_range(range).unwrap(), addr).unwrap();

        self.regions.push(region);

        Ok(())
    }

    fn map_foreign_region(&mut self, domid: u16) -> Result<()> {
        self.map_region(
            GuestAddress(GUEST_RAM0_BASE),
            self.guest_size,
            "/dev/xen/privcmd",
            MmapXenFlags::FOREIGN.bits(),
            domid as u32,
        )
    }

    // Maps entire guest address space in one region.
    //
    // The address received here is special as the kernel ORs the address with 0x8000000000000000
    // to mark it for grant mapping. If the memory mapping fails for a device here and address
    // doesn't have the top bit set, then either the guest kernel's DT doesn't have the required
    // iommu nodes or it is missing some Kconfig options.
    //
    // Hint: XEN_GRANT_DMA_ADDR_OFF in drivers/xen/grant-dma-ops.c.
    fn map_grant_region(&mut self, addr: u64, size: usize, domid: u16, flags: u32) -> Result<()> {
        if size == 0 {
            return Ok(());
        }

        self.map_region(
            GuestAddress(addr),
            size,
            "/dev/xen/gntdev",
            flags | MmapXenFlags::GRANT.bits(),
            domid as u32,
        )
    }

    // Maps virtqueues in advance.
    fn map_grant_queue_regions(&mut self, queue: &Queue, vq_size: usize, domid: u16) -> Result<()> {
        let mut size = vq_size * std::mem::size_of::<Descriptor>();
        self.map_grant_region(queue.desc_table(), size, domid, 0)?;

        size = vq_size * std::mem::size_of::<__virtio16>();
        size += std::mem::size_of::<vring_avail>();
        // Extra 2 bytes for vring_used_elem at the end of avail ring
        size += std::mem::size_of::<__virtio16>();
        self.map_grant_region(queue.avail_ring(), size, domid, 0)?;

        size = vq_size * std::mem::size_of::<vring_used_elem>();
        size += std::mem::size_of::<vring_used>();
        // Extra 2 bytes for vring_used_elem at the end of used ring
        size += std::mem::size_of::<__virtio16>();
        self.map_grant_region(queue.used_ring(), size, domid, 0)?;

        Ok(())
    }

    // Maps non-virtqueues memory with no advance map flag.
    fn map_grant_remaining_regions(&mut self, domid: u16) -> Result<()> {
        // Sort the already added regions by start address.
        self.sort_regions();

        let mut regions: Vec<GuestRegionMmap> = self.regions.drain(..).collect();
        let mut offset = XEN_GRANT_ADDR_OFF;

        for region in &regions {
            let size = (region.start_addr().0 - offset) as usize;
            self.map_grant_region(offset, size, domid, MmapXenFlags::NO_ADVANCE_MAP.bits())?;
            offset = region.start_addr().0 + region.len() + XC_PAGE_SIZE as u64 - 1;
            offset = (offset >> XC_PAGE_SHIFT) << XC_PAGE_SHIFT;
        }

        // Regions are mapped from address 0 until end of all virtqueues, lets map the rest now.
        self.map_grant_region(
            offset,
            self.guest_size - (offset - XEN_GRANT_ADDR_OFF) as usize,
            domid,
            MmapXenFlags::NO_ADVANCE_MAP.bits(),
        )?;
        self.regions.append(&mut regions);

        // Sort the already added regions by start address.
        self.sort_regions();

        Ok(())
    }

    fn init_vq(&mut self, domid: u16, index: usize) -> Result<()> {
        let vq = &mut self.vq[index];
        let kick = vq.kick.try_clone().unwrap();
        let vq_size = vq.size;

        if vq.desc == 0 || vq.avail == 0 || vq.used == 0 {
            panic!();
        }

        let mut queue = Queue::new(vq_size as u16).unwrap();
        queue.set_desc_table_address(
            Some((vq.desc & 0xFFFFFFFF) as u32),
            Some((vq.desc >> 32) as u32),
        );
        queue.set_avail_ring_address(
            Some((vq.avail & 0xFFFFFFFF) as u32),
            Some((vq.avail >> 32) as u32),
        );
        queue.set_used_ring_address(
            Some((vq.used & 0xFFFFFFFF) as u32),
            Some((vq.used >> 32) as u32),
        );
        queue.set_next_avail(0);

        vq.ready = 1;

        if !self.foreign_mapping {
            self.map_grant_queue_regions(&queue, vq_size as usize, domid)?;
        }

        self.queues.push((index, queue, kick));

        Ok(())
    }

    fn destroy_vq(&mut self) {
        self.queues.drain(..);
    }

    fn mem(&mut self) -> GuestMemoryAtomic<GuestMemoryMmap> {
        GuestMemoryAtomic::new(
            GuestMemoryMmap::from_regions(self.regions.drain(..).collect()).unwrap(),
        )
    }

    fn activate_device(&mut self, dev: &XenDevice, domid: u16) -> Result<()> {
        // Map rest of the memory, now that all the queues are mapped.
        if !self.foreign_mapping {
            self.map_grant_remaining_regions(domid)?;
        }

        dev.gdev
            .lock()
            .unwrap()
            .activate(self.mem(), dev.interrupt(), self.queues.drain(..).collect())
            .map_err(Error::VhostFrontendActivateError)
    }

    pub fn io_event(&mut self, ioreq: &mut ioreq, dev: &XenDevice) -> Result<()> {
        let offset = ioreq.addr - self.addr;

        match ioreq.dir() as u32 {
            IOREQ_READ => self.io_read(ioreq, offset),
            IOREQ_WRITE => self.io_write(ioreq, dev, offset),
            _ => Err(Error::InvalidMmioDir(ioreq.dir())),
        }
    }
}

impl Drop for XenMmio {
    fn drop(&mut self) {
        let xfm = self.guest.xfm.lock().unwrap();
        let ioreq = xfm.ioreq(0).unwrap();
        let xec = self.guest.xec.lock().unwrap();

        if let Some(handle) = self.handle.lock().unwrap().take() {
            handle.join().unwrap();
        }

        for (index, vq) in self.vq.iter().enumerate() {
            let kick = vq.kick.try_clone().unwrap();

            self.guest
                .xdm
                .lock()
                .unwrap()
                .set_ioeventfd(&kick, ioreq, xec.ports(), self.addr, index as u32, false)
                .unwrap();
        }
    }
}
