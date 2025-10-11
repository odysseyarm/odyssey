OUTPUT_DIR = build
APP_TARGET_DIR = ../../target
APP_ARTIFACT_DIR = $(APP_TARGET_DIR)/thumbv7em-none-eabihf/release

BOOT_DIR = ../bootloaders/vm
BOOT_TARGET_DIR = $(BOOT_DIR)/target
BOOT_TRIPLE = thumbv7em-none-eabihf
BOOT_ARTIFACT_DIR = $(BOOT_TARGET_DIR)/$(BOOT_TRIPLE)/release
BOOT_ELF = $(BOOT_ARTIFACT_DIR)/bootloader
BOOT_HEX = $(OUTPUT_DIR)/bootloader.hex

.PHONY: all
all: $(OUTPUT_DIR)/merged.hex

$(OUTPUT_DIR):
	mkdir -p "$(OUTPUT_DIR)"

$(APP_ARTIFACT_DIR): $(shell find . -path ./$(APP_TARGET_DIR) -prune -o -print)
	cp ../../memory/vm.x ../../memory.x
	cargo build --release --target thumbv7em-none-eabihf

$(OUTPUT_DIR)/app.elf: $(APP_ARTIFACT_DIR) | $(OUTPUT_DIR)
	cp $(APP_ARTIFACT_DIR)/app $@

$(BOOT_ELF): $(shell find $(BOOT_DIR) -maxdepth 2 -name Cargo.toml -o -name src -o -name memory.x -o -name build.rs)
	cd $(BOOT_DIR) && cargo build --release --target $(BOOT_TRIPLE)

$(OUTPUT_DIR)/%.hex: $(OUTPUT_DIR)/%.elf
	llvm-objcopy -O ihex $^ $@

$(BOOT_HEX): $(BOOT_ELF) | $(OUTPUT_DIR)
	llvm-objcopy -O ihex $(BOOT_ELF) $(BOOT_HEX)

$(OUTPUT_DIR)/app.hex: $(OUTPUT_DIR)/app.elf
	llvm-objcopy -O ihex $^ $@

$(OUTPUT_DIR)/merged.hex: $(BOOT_HEX) $(OUTPUT_DIR)/app.hex
	mergehex -o $@ -m $^

.PHONY: cleanall
cleanall:
	rm -rf $(OUTPUT_DIR) $(APP_TARGET_DIR)

.PHONY: clean
clean:
	rm -rf $(OUTPUT_DIR)

.PHONY: flash
flash: $(OUTPUT_DIR)/merged.hex
	nrfjprog --program $(OUTPUT_DIR)/merged.hex --sectorerase --verify --reset

.PHONY: flash-chiperase
flash-chiperase: $(OUTPUT_DIR)/merged.hex
	nrfjprog --program $(OUTPUT_DIR)/merged.hex --chiperase --verify --reset

.PHONY: flash-recover
flash-recover: $(OUTPUT_DIR)/merged.hex
	nrfjprog --program $(OUTPUT_DIR)/merged.hex --recover --verify --reset

.PHONY: flash-app
flash-app: $(OUTPUT_DIR)/app.hex
	nrfjprog --program $(OUTPUT_DIR)/app.hex --sectorerase --verify --reset

.PHONY: reset
reset:
	nrfjprog --reset

.PHONY: attach-app
attach-app:
	probe-rs attach --chip nRF5340_xxAA "$(OUTPUT_DIR)/app.elf"
