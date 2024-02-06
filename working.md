# Control flow

main.rs:
 - Wait for changes to "backend/virtio/<Guest Num>/<Device Num>" path in XS.
   - Detects new guests and devices.
     - If guest is new:
	- Setup ioreq server
	- Setup event channel between guest and Dom0
	  - Create thread (Thread A) to wait for events on this.
     - Add device:
       - Reads basic device information from XS store, like addr, irq line, etc.
       - Creates a new generic device with vhost-user-frontend crate.
	 - If foreign memory mapping is enabled, the entire guest space is
	   mapped here.
	 - device::map_io_range_to_ioreq_server(self.addr, VIRTIO_MMIO_IO_SIZE) to start
	   things over - virtio negotiations.

Thread A (guest::io_event()):
- All virtio transactions are managed here.
- Finds the device responsible for ioreq event.
- After few negotiations, VIRTIO_MMIO_QUEUE_READY event is received from guest.
- By this point virtqueue details are already sent from guest.
- If grant memory mapping is selected, map the memory for virtqueues here.
- Activate the vhost-user-frontend device now.
- The backend will get notified and will start vhost-user negotiations.
