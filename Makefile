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
# `make doom-iso` sets this, which links the GPL-2 DOOM engine in doom/.
FEATURES   ?=
CARGO_FLAGS += $(if $(FEATURES),--features $(FEATURES),)
DOOM_WAD   := $(BUILD)/doom.wad

QEMU       := qemu-system-x86_64
# An AC'97 codec, which is what HALCYON's audio driver knows how to talk to.
QEMU_AUDIO := -device AC97
QEMU_COMMON := -m 512M -serial stdio -no-reboot $(QEMU_AUDIO) -cdrom $(ISO)
OVMF_CODE  := /usr/share/OVMF/OVMF_CODE_4M.fd
OVMF_VARS  := /usr/share/OVMF/OVMF_VARS_4M.fd

.PHONY: all kernel iso run run-uefi smoke smoke-doom screenshots clean fmt check doom-iso run-doom wad

all: iso

kernel:
	cd $(KERNEL_DIR) && cargo build $(CARGO_FLAGS)
	@echo "--- kernel ELF ---"
	@readelf -h $(notdir $(KERNEL_ELF)) >/dev/null 2>&1 || true
	@size $(KERNEL_ELF) 2>/dev/null || true

$(KERNEL_ELF): kernel

# Recursive: `wildcard initrd/*` would miss edits inside initrd/help, because
# changing a file's contents does not touch its directory's timestamp.
INITRD_FILES := $(shell find initrd -type f 2>/dev/null)

$(INITRD): $(INITRD_FILES) | $(BUILD)
	tools/mkinitrd.sh initrd $(INITRD)

$(BUILD):
	mkdir -p $(BUILD)

iso: kernel $(INITRD)
	@rm -rf $(ISO_ROOT)
	@mkdir -p $(ISO_ROOT)/boot/grub
	cp $(KERNEL_ELF) $(ISO_ROOT)/boot/halcyon.elf
	cp $(INITRD) $(ISO_ROOT)/boot/initrd.tar
	cp iso/boot/grub/grub.cfg $(ISO_ROOT)/boot/grub/grub.cfg
	@if [ -n "$(FEATURES)" ] && [ -s $(DOOM_WAD) ]; then \
		cp $(DOOM_WAD) $(ISO_ROOT)/boot/doom.wad; \
		sed -i 's|module2 /boot/initrd.tar initrd|module2 /boot/initrd.tar initrd\n    module2 /boot/doom.wad doom|' \
			$(ISO_ROOT)/boot/grub/grub.cfg; \
		echo "iso: including $(DOOM_WAD)"; \
	fi
	grub-mkrescue -o $(ISO) $(ISO_ROOT) 2>/dev/null
	@echo
	@echo "ISO: $(ISO) ($$(du -h $(ISO) | cut -f1))"
	@sha256sum $(ISO)

# Fetch an IWAD (Freedoom unless you supply your own) into build/doom.wad.
wad:
	tools/fetch-doom-wad.sh $(DOOM_WAD)

# An ISO with DOOM. The combined kernel is GPL-2; see doom/README.md.
doom-iso: wad
	$(MAKE) iso FEATURES=doom ISO=$(BUILD)/halcyon-doom.iso

run-doom: doom-iso
	$(QEMU) -m 1G -serial stdio -no-reboot $(QEMU_AUDIO) -cdrom $(BUILD)/halcyon-doom.iso

run: iso
	$(QEMU) $(QEMU_COMMON)

run-uefi: iso
	@cp $(OVMF_VARS) $(BUILD)/OVMF_VARS.fd
	$(QEMU) $(QEMU_COMMON) \
		-drive if=pflash,format=raw,unit=0,file=$(OVMF_CODE),readonly=on \
		-drive if=pflash,format=raw,unit=1,format=raw,file=$(BUILD)/OVMF_VARS.fd

smoke: iso
	python3 tools/smoke.py --iso $(ISO) --firmware bios --audio
	python3 tools/smoke.py --iso $(ISO) --firmware uefi --audio

# Boot DOOM, play a few seconds, and assert the captured audio is not silence.
smoke-doom: doom-iso
	python3 tools/smoke.py --iso $(BUILD)/halcyon-doom.iso --firmware bios \
		--memory 1G --boot-wait 26 --expect DOOM-RUNNING \
		--expect POINTER-GRABBED --expect POINTER-RELEASED \
		--script tools/scripts/doom.txt \
		--audio-wav $(BUILD)/doom-audio.wav

screenshots: iso
	python3 tools/smoke.py --iso $(ISO) --firmware bios --screenshots $(BUILD)/shots

check:
	cd $(KERNEL_DIR) && cargo clippy $(CARGO_FLAGS) 2>/dev/null || cd $(KERNEL_DIR) && cargo check $(CARGO_FLAGS)

fmt:
	cd $(KERNEL_DIR) && cargo fmt

clean:
	cd $(KERNEL_DIR) && cargo clean
	rm -rf $(BUILD)
