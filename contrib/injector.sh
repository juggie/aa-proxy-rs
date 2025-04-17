#!/bin/sh

# download official images
for pi in 0w 3a 4 zero2w; do
	wget https://github.com/nisargjhaveri/WirelessAndroidAutoDongle/releases/latest/download/raspberrypi${pi}-sdcard.img.xz
done

# unpack
for filename in *xz; do
	echo ">>> unpacking $filename..."
	unxz $filename
done

# process each image and place an aa-proxy-rs
for filename in *img; do
	echo ">>> processing $filename..."
	mount -o loop,offset=33554944 $filename /mnt
	rm /mnt/etc/init.d/S93aawgd
	cp /root/S93aa-proxy-rs /mnt/etc/init.d

	# pi zero w has different binary
	if [ $filename = "raspberrypi0w-sdcard.img" ]; then
		cp /root/aa-proxy-rs-0w /mnt/usr/bin/aa-proxy-rs
	else
		cp /root/aa-proxy-rs /mnt/usr/bin
	fi

	umount /mnt

	echo ">>> compressing $filename..."
	xz $filename
done
