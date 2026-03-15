.PHONY: build run run-serial clean

build:
	cargo bootimage

run:
	qemu-system-x86_64 \
		-drive format=raw,file=target/x86_64-unknown-none/debug/bootimage-merlion-kernel.bin \
		-serial stdio

run-serial:
	qemu-system-x86_64 \
		-drive format=raw,file=target/x86_64-unknown-none/debug/bootimage-merlion-kernel.bin \
		-display none \
		-serial stdio

clean:
	cargo clean
