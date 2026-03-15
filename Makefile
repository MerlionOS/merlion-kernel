.PHONY: build run clean

build:
	cargo bootimage

run:
	qemu-system-x86_64 -drive format=raw,file=target/x86_64-unknown-none/debug/bootimage-merlion-kernel.bin

clean:
	cargo clean
