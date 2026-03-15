.PHONY: build run run-serial run-fullscreen test clean

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

run-fullscreen:
	qemu-system-x86_64 \
		-drive format=raw,file=target/x86_64-unknown-none/debug/bootimage-merlion-kernel.bin \
		-serial stdio \
		-full-screen

test:
	cargo test --test basic

clean:
	cargo clean
