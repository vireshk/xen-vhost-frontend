// Copyright 2022-2023 Linaro Ltd. All Rights Reserved.
//          Viresh Kumar <viresh.kumar@linaro.org>
//
// SPDX-License-Identifier: Apache-2.0

use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    mem,
    os::unix::io::AsRawFd,
    sync::Arc,
    sync::Mutex,
    thread::{Builder, JoinHandle},
};

use vhost::vhost_user::message::VhostUserProtocolFeatures;
use vhost_user_frontend::{Generic, VirtioDevice};
use vhost_user_frontend::{GuestMemoryMmap, GuestRegionMmap};
use virtio_bindings::virtio_config::{VIRTIO_F_IOMMU_PLATFORM, VIRTIO_F_VERSION_1};
use virtio_queue::{Queue, QueueT};
use vm_memory::ByteValued;
use vm_memory::{guest_memory::FileOffset, GuestAddress, GuestMemoryAtomic};

use vmm_sys_util::eventfd::{EventFd, EFD_NONBLOCK};

use super::{device::XenDevice, epoll::XenEpoll, Error, Result};

// Virtio messages
const VIRTIO_MSG_DEVICE_INFO: u8 = 0x02;
const VIRTIO_MSG_GET_DEV_FEATURES: u8 = 0x03;
const VIRTIO_MSG_SET_DRV_FEATURES: u8 = 0x04;
const VIRTIO_MSG_GET_CONFIG: u8 = 0x05;
const VIRTIO_MSG_SET_CONFIG: u8 = 0x06;
const VIRTIO_MSG_GET_DEVICE_STATUS: u8 = 0x07;
const VIRTIO_MSG_SET_DEVICE_STATUS: u8 = 0x08;
const VIRTIO_MSG_GET_VQUEUE: u8 = 0x09;
const VIRTIO_MSG_SET_VQUEUE: u8 = 0x0a;
const VIRTIO_MSG_RESET_VQUEUE: u8 = 0x0b;
//const VIRTIO_MSG_GET_SHM: u8 = 0x0c;
//const VIRTIO_MSG_EVENT_CONFIG: u8 = 0x40;
const VIRTIO_MSG_EVENT_AVAIL: u8 = 0x41;
const VIRTIO_MSG_EVENT_USED: u8 = 0x42;

const VIRTIO_MSG_TYPE_RESPONSE: u8 = 0x1;
const VIRTIO_MSG_TYPE_TRANSPORT: u8 = 0x0;
//const VIRTIO_MSG_TYPE_BUS: u8 = 0x2;

const VIRTIO_MSG_MIN_SIZE: usize = 50;
//const VIRTIO_MSG_MAX_SIZE: u8 = 65536;
//const VIRTIO_MSG_REVISION_1: u8 = 0x1;
// const VIRTIO_MSG_EVENT_AVAIL_WRAP_SHIFT: u8 = 31;

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct GetDeviceInfoResp {
    device_id: u32,
    vendor_id: u32,
    num_feature_bits: u32,
    config_size: u32,
    max_vq_count: u32,
    admin_vq_start_idx: u16,
    admin_vq_count: u16,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct GetFeatures {
    index: u32,
    num: u32,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct GetFeaturesResp {
    index: u32,
    num: u32,
    features: u64,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct SetFeatures {
    index: u32,
    num: u32,
    features: u64,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct GetConfig {
    offset: u32,
    size: u32,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct GetConfigResp {
    generation: u32,
    offset: u32,
    size: u32,
    config: u64,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct SetConfig {
    generation: u32,
    offset: u32,
    size: u32,
    config: u64,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct SetConfigResp {
    generation: u32,
    offset: u32,
    size: u32,
    config: u64,
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
struct SetDeviceStatusResp {
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
    reserved: u32,
    descriptor_addr: u64,
    driver_addr: u64,
    device_addr: u64,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct SetVqueue {
    index: u32,
    reserved0: u32,
    size: u32,
    reserved1: u32,
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
    generation: u32,
    offset: u32,
    size: u32,
    config: u64,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct EventAvail {
    index: u32,
    next_offset_wrap: u32,
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct EventUsed {
    index: u32,
}

#[derive(Copy, Clone)]
#[repr(C, packed)]
union ReqRespTypes {
    payload: [u8; VIRTIO_MSG_MIN_SIZE],

    // virtio messages
    get_device_info_resp: GetDeviceInfoResp,
    get_features: GetFeatures,
    get_features_resp: GetFeaturesResp,
    set_features: SetFeatures,
    get_config: GetConfig,
    get_config_resp: GetConfigResp,
    set_config: SetConfig,
    set_config_resp: SetConfigResp,
    get_device_status_resp: GetDeviceStatusResp,
    set_device_status: SetDeviceStatus,
    set_device_status_resp: SetDeviceStatusResp,
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
        Self { payload: [0; VIRTIO_MSG_MIN_SIZE] }
    }
}

#[derive(Copy, Clone, Default)]
#[repr(C, packed)]
struct VirtioMsg {
    _type: u8,
    id: u8,
    dev_id: u16,
    token: u16,
    msg_size: u16,
    r: ReqRespTypes,
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
    vendor_id: u32,
    status: u32,
    queues_count: usize,
    queues: Vec<(usize, Queue, EventFd)>,
    vq: Vec<VirtQueue>,
    regions: Vec<GuestRegionMmap>,
    request: VirtioMsg,
    response: VirtioMsg,
    respond: bool,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl XenMmio {
    pub fn new(gdev: &Generic) -> Result<Self> {
        let sizes = gdev.queue_max_sizes();

        let mut mmio = Self {
            vendor_id: 0x4d564b4c,
            status: 0,
            queues_count: sizes.len(),
            queues: Vec::with_capacity(sizes.len()),
            vq: Vec::new(),
            regions: Vec::new(),
            request: VirtioMsg::default(),
            response: VirtioMsg::default(),
            respond: false,
            handle: Mutex::new(None),
        };

        for (_, size) in sizes.iter().enumerate() {
            let kick = EventFd::new(EFD_NONBLOCK).unwrap();

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
                    let epoll = XenEpoll::new(vec![file.as_raw_fd()]).unwrap();

                    while let Ok(_) = epoll.wait() {
                        file.read(request).unwrap();

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

        request._type = VIRTIO_MSG_TYPE_TRANSPORT;
        request.id = VIRTIO_MSG_EVENT_USED;
        request.msg_size = VIRTIO_MSG_MIN_SIZE as u16;
        request.token = 0;

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

    fn config_read(&self, gdev: &Generic, offset: u32, size: u32) -> Result<u64> {
        let mut data: u64 = 0;
        gdev.read_config(offset as u64, &mut data.as_mut_slice()[0..size as usize]);

        Ok(data)
    }

    fn config_write(&self, gdev: &mut Generic, data: u64, offset: u32, size: u32) -> Result<()> {
        gdev.write_config(offset as u64, &data.to_ne_bytes()[0..size as usize]);
        Ok(())
    }

    fn handle_virtio_msg(&mut self, dev: &XenDevice) -> Result<()> {
        self.response.dev_id = self.request.dev_id;

        match self.request.id {
            VIRTIO_MSG_DEVICE_INFO => {
                let gdev = &mut dev.gdev.lock().unwrap();

                self.response.r.get_device_info_resp.device_id = gdev.device_type();
                self.response.r.get_device_info_resp.vendor_id = self.vendor_id;
                self.response.r.get_device_info_resp.num_feature_bits = 64;
                self.response.r.get_device_info_resp.config_size = 0x200;
                self.response.r.get_device_info_resp.max_vq_count = 0;
                self.response.r.get_device_info_resp.admin_vq_start_idx = 0;
                self.response.r.get_device_info_resp.admin_vq_count = 0;
                self.respond = true;
            }

            VIRTIO_MSG_GET_DEV_FEATURES => {
                let gdev = &mut dev.gdev.lock().unwrap();

                let mut features = gdev.device_features();
                features |= 1 << VIRTIO_F_VERSION_1;
                features |= 1 << VIRTIO_F_IOMMU_PLATFORM;

                unsafe {
                    self.response.r.get_features_resp.index = self.request.r.get_features.index;
                    self.response.r.get_features_resp.features = features;
                    self.response.r.get_features_resp.num = 1;
                }
                self.respond = true;
            }

            VIRTIO_MSG_SET_DRV_FEATURES => {
                let gdev = &mut dev.gdev.lock().unwrap();

                let driver_features = unsafe { self.request.r.set_features.features };

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
                    return Err(Error::InvalidSize(size as u8));
                }

                let offset = unsafe { self.request.r.get_config.offset};

                let config = self.config_read(gdev, offset, size)?;

                unsafe {
                    self.response.r.get_config_resp.config = config;
                    self.response.r.get_config_resp.offset = self.request.r.get_config.offset;
                    self.response.r.get_config_resp.size = self.request.r.get_config.size;
                    self.response.r.get_config_resp.generation = 1;
                }
                self.respond = true;
            }

            VIRTIO_MSG_SET_CONFIG => {
                let gdev = &mut dev.gdev.lock().unwrap();

                let size = unsafe { self.request.r.set_config.size };

                if size == 0 || size > 8 {
                    return Err(Error::InvalidSize(size as u8));
                }

                let offset = unsafe { self.request.r.set_config.offset };
                let config = unsafe { self.request.r.set_config.config };

                self.config_write(
                    gdev,
                    config,
                    offset,
                    size,
                )?;

                // Linux doesn't use below, still send it.
                unsafe {
                    self.response.r.set_config_resp.config = self.request.r.set_config.config;
                    self.response.r.set_config_resp.offset = self.request.r.set_config.offset;
                    self.response.r.set_config_resp.size = self.request.r.set_config.size;
                    self.response.r.set_config_resp.generation = self.request.r.set_config.generation;
                }
                self.respond = true;
            }

            VIRTIO_MSG_GET_DEVICE_STATUS => {
                self.response.r.get_device_status_resp.status = self.status;
                self.respond = true;
            }

            VIRTIO_MSG_SET_DEVICE_STATUS => {
                unsafe { self.status = self.request.r.set_device_status.status };
                self.response.r.set_device_status_resp.status = self.status;
                self.respond = true;
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
                self.init_vq(index as usize)?;

                // Wait for all virtqueues to get initialized.
                if self.queues.len() == self.queues_count {
                    self.map_mem(self.vq[0].desc)?;

                    self.activate_device(dev)?;
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
        self.response.msg_size = VIRTIO_MSG_MIN_SIZE as u16;
        self.response.token = self.request.token;

        match self.request._type {
            VIRTIO_MSG_TYPE_TRANSPORT => self.handle_virtio_msg(dev),
            _ => Err(Error::InvalidReqType(self.request._type)),
        }
    }

    fn map_mem(&mut self, addr: u64) -> Result<()> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/virtio-msg-lb")
            .unwrap();

        let size = 0x800000; // Fix the size to 8 MB for now

        let region = GuestRegionMmap::from_range(
            GuestAddress(addr),
            size,
            Some(FileOffset::new(file, 0)),
        ).unwrap();

        self.regions.push(region);

        Ok(())
    }

    fn init_vq(&mut self, index: usize) -> Result<()> {
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

    fn activate_device(&mut self, dev: &XenDevice) -> Result<()> {
        let mem = self.mem();
        dev.gdev
            .lock()
            .unwrap()
            .activate(mem, dev.interrupt(), self.queues.drain(..).collect())
            .map_err(Error::VhostFrontendActivateError)
    }
}

impl Drop for XenMmio {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.lock().unwrap().take() {
            handle.join().unwrap();
        }
    }
}
