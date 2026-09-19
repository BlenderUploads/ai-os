# HALCYON build system.
#
#   make            build the kernel and the bootable ISO
#   make run        boot the ISO in QEMU (legacy BIOS)
#   make run-uefi   boot the ISO in QEMU (64-bit UEFI, via OVMF)
#   make smoke      headless boot test + screenshots
#   make clean

SHELL      := /bin/bash
PROFILE    ?= release
KERNEL_DIR := kernel
TARGET     := x86_64-unknown-none
KERNEL_ELF := $(KERNEL_DIR)/target/$(TARGET)/$(PROFILE)/halcyon
BUILD      := build
ISO_ROOT   := $(BUILD)/isoroot
ISO        := $(BUILD)/halcyon.iso
INITRD     := $(BUILD)/initrd.tar

CARGO_FLAGS := $(if $(filter release,$(PROFILE)),--release,)

QEMU       := qemu-system-x86_64
QEMU_COMMON := -m 512M -serial stdio -no-reboot -cdrom $(ISO)
OVMF_CODE  := /usr/share/OVMF/OVMF_CODE_4M.fd
OVMF_VARS  := /usr/share/OVMF/OVMF_VARS_4M.fd

.PHONY: all kernel iso run run-uefi smoke screenshots clean fmt check

all: iso

kernel:
	cd $(KERNEL_DIR) && cargo build $(CARGO_FLAGS)
	@echo "--- kernel ELF ---"
	@readelf -h $(notdir $(KERNEL_ELF)) >/dev/null 2>&1 || true
	@size $(KERNEL_ELF) 2>/dev/null || true

$(KERNEL_ELF): kernel

$(INITRD): $(wildcard initrd/*) | $(BUILD)
	tools/mkinitrd.sh initrd $(INITRD)

$(BUILD):
	mkdir -p $(BUILD)

iso: kernel $(INITRD)
	@rm -rf $(ISO_ROOT)
	@mkdir -p $(ISO_ROOT)/boot/grub
	cp $(KERNEL_ELF) $(ISO_ROOT)/boot/halcyon.elf
	cp $(INITRD) $(ISO_ROOT)/boot/initrd.tar
	cp iso/boot/grub/grub.cfg $(ISO_ROOT)/boot/grub/grub.cfg
	grub-mkrescue -o $(ISO) $(ISO_ROOT) 2>/dev/null
	@echo
	@echo "ISO: $(ISO) ($$(du -h $(ISO) | cut -f1))"
	@sha256sum $(ISO)

run: iso
	$(QEMU) $(QEMU_COMMON)

run-uefi: iso
	@cp $(OVMF_VARS) $(BUILD)/OVMF_VARS.fd
	$(QEMU) $(QEMU_COMMON) \
		-drive if=pflash,format=raw,unit=0,file=$(OVMF_CODE),readonly=on \
		-drive if=pflash,format=raw,unit=1,format=raw,file=$(BUILD)/OVMF_VARS.fd

smoke: iso
	python3 tools/smoke.py --iso $(ISO) --firmware bios
	python3 tools/smoke.py --iso $(ISO) --firmware uefi

screenshots: iso
	python3 tools/smoke.py --iso $(ISO) --firmware bios --screenshots $(BUILD)/shots

check:
	cd $(KERNEL_DIR) && cargo clippy $(CARGO_FLAGS) 2>/dev/null || cd $(KERNEL_DIR) && cargo check $(CARGO_FLAGS)

fmt:
	cd $(KERNEL_DIR) && cargo fmt

clean:
	cd $(KERNEL_DIR) && cargo clean
	rm -rf $(BUILD)
