# `xen-vhost-frontend`

## Description
This program implements Xen specific `vhost-user-frontend`. This is PoC
(proof-of-concept) implementation that lets us verify hypervisor-agnosticism of
Rust based `vhost-user` backends.

This is only tested for `AARCH64` currently.

## Key components

- [xen-vhost-frontend](https://github.com/vireshk/xen-vhost-frontend/tree/main)

  This is the Xen specific implementation of the `vhost-user-frontend` crate.
  This is based on the EPAM's
  [virtio-disk](https://github.com/xen-troops/virtio-disk) implementation.

- [Xen](https://github.com/vireshk/xen/tree/master)

  Enable following config options:

  ```
  diff --git a/xen/arch/arm/configs/arm64_defconfig b/xen/arch/arm/configs/arm64_defconfig
  index e69de29bb2d1..38ca05a8b416 100644
  --- a/xen/arch/arm/configs/arm64_defconfig
  +++ b/xen/arch/arm/configs/arm64_defconfig
  @@ -0,0 +1,3 @@
  +CONFIG_IOREQ_SERVER=y
  +CONFIG_EXPERT=y

  ```

  There are few patches at the top of this tree which are required to make Xen
  grant mappings works, which aren't merged in upstream Xen tree yet.

  xen-vhost-frontend accepts two arguments as of now, "socket-path" and
  "foreign-mapping".

  "socket-path" is the path where the socket will be present. xen-vhost-frontend
  looks for a socket named in following format at this path:
  "<device-name>.sock<N>", where device-name is the name that is defined in
  "src/supported_devices.rs" and N is the index number (starts from 0) of the
  device created, as you can create multiple instances of the same device for a
  guest.

  "foreign-mapping" is of boolean type. If present, the memory regions created
  by xen-vhost-frontend will be of type xen-foreign memory, which maps the
  entire guest space in advance. With this, the guest configurations shouldn't
  contain "grant_usage=enabled" parameter as we need guest to send foreign
  memory regions.

  When "foreign-mapping" is not present in the arguments, the memory regions
  created by xen-vhost-frontend are of type xen-grant memory, where the memory
  is mapped/unmapped on the fly, as and when required. With this, the guest
  configuration should contain "grant_usage=enabled" parameter, as we need the
  guest to send grant memory regions. This parameter is only required when
  backend is running in Dom0, else this can be skipped if the backend is running
  in any of the domUs.

  xen-vhost-frontend currently supports I2C, FS, and GPIO backends. You can add
  support for more devices by adding a relevant entry in
  `src/supported_devices.rs` file. You would also need to update the following
  structure with number and size of virtqueues:
  https://github.com/vireshk/vhost/blob/main/crates/vhost-user-frontend/src/lib.rs#L185.

- [vhost-device](https://github.com/vireshk/vhost-device/tree/main)

  These are Rust based `vhost-user` backends, maintained inside the rust-vmm
  project. These are truly hypervisor-agnostic.

  Xen grant-mapping work is in progress and isn't upstreamed yet. The necessary
  changes are updated in Viresh's tree for now.

- [Linux Kernel](https://git.kernel.org/pub/scm/linux/kernel/git/vireshk/linux.git/log/?h=xen/host)

  Though the setup works fine on vanilla kernel, this branch enables the
  necessary config options to make it all work. The same image can be used for
  both host and guest kernels. User needs v6.3-rc1 or later, as it contains a
  fix for xen grant mappings.

  The following kernel config options must be enabled for Xen grant and foreign
  mappings: CONFIG_XEN_GNTDEV, CONFIG_XEN_GRANT_DEV_ALLOC, CONFIG_XEN_PRIVCMD,
  CONFIG_XEN_GRANT_DMA_IOMMU, and CONFIG_XEN_VIRTIO.


## Test Setup

The following steps lets one test I2C `vhost-device` on Xen.

- Build Xen for aarch64:

  ```
  $ ./configure --libdir=/usr/lib --build=x86_64-unknown-linux-gnu --host=aarch64-linux-gnu \
    --disable-docs --disable-golang --disable-ocamltools \
    --with-system-qemu=/root/qemu/build/i386-softmmu/qemu-system-i386
  
  $ make -j9 debball CROSS_COMPILE=aarch64-linux-gnu- XEN_TARGET_ARCH=arm64
  ```

- Run Xen via Qemu on X86 machine:

  ```
  $ qemu-system-aarch64 -machine virt,virtualization=on -cpu cortex-a57 -serial mon:stdio \
    -device virtio-net-pci,netdev=net0 -netdev user,id=net0,hostfwd=tcp::8022-:22 \
    -drive file=/home/debian-bullseye-arm64.qcow2,index=0,id=hd0,if=none,format=qcow2 \
    -device virtio-scsi-pci -device scsi-hd,drive=hd0 \
    -display none -m 8192 -smp 8 -kernel /home/xen/xen \
    -append "dom0_mem=5G,max:5G dom0_max_vcpus=7 loglvl=all guest_loglvl=all" \
    -device guest-loader,addr=0x46000000,kernel=/home/Image,bootargs="root=/dev/sda2 console=hvc0 earlyprintk=xen" \
    -device ds1338,address=0x20
  ```
  The `ds1338` entry here is required to create a virtual I2C based RTC device
  on Dom0.

  This should get Dom0 up and running.

- Build `xen-vhost-frontend` crate:

  ```
  $ git clone https://github.com/vireshk/xen-vhost-frontend
  $ cd xen-vhost-frontend
  $ cargo build --release
  $ cd ../
  ```

- Build `vhost-device` crate:

  ```
  $ git clone https://github.com/rust-vmm/vhost-device
  $ cd vhost-device
  $ cargo build --release
  $ cd ../
  ```

- Setup I2C based RTC devices on Dom0

  This is required to control the device on Dom0 from the guest.

  ```
  $ echo ds1338 0x20 > /sys/bus/i2c/devices/i2c-0/new_device
  $ echo 0-0020 > /sys/bus/i2c/devices/0-0020/driver/unbind
  ```

- Lets run everything

  First start the I2C backend. This tells the I2C backend to hook up to
  `/root/i2c.sock0` socket and wait for the master to start transacting. The
  I2C controller used here on Dom0 is named `90c0000.i2c` (can be read from
  `/sys/bus/i2c/devices/i2c-0/name`) and `32` here matches the device on I2C bus
  set in the previous commands (`0x20`).

  ```
  $ /root/vhost-device/target/release/vhost-device-i2c -s /root/i2c.sock -c 1 -l 90c0000.i2c:32'
  ```

  Then start xen-vhost-frontend, by providing path of the socket to the master
  side. This by default will create grant-mapping for the memory regions.

  ```
  $ /root/xen-vhost-frontend/target/release/xen-vhost-frontend --socket-path /root/'
  ```

  Now that all the preparations are done, lets start the guest. The guest kernel
  should have Virtio related config options enabled, along with `i2c-virtio`
  driver.

  ```
  $ xl create -c domu.conf
  ```

  The guest should boot now. Once the guest is up, you can create the I2C based
  RTC device and use it. Following will create `/dev/rtc0` in the guest, which you
  can configure with the standard `hwclock` utility.

  ```
  $ echo ds1338 0x20 > /sys/bus/i2c/devices/i2c-0/new_device
  ```

## Sample domu.conf

  ```
  kernel="/root/Image"
  memory=512
  vcpus=3
  command="console=hvc0 earlycon=xenboot"
  name="domu"
  virtio = [ "type=virtio,device22, transport=mmio, grant_usage=enabled" ]
  ```

  The device type here defines the device to be emulated on the guest. The type
  value is set with the DT `compatible` string of the device. For example,
  it is `virtio,device22` for I2C and `virtio,device29` for GPIO.
