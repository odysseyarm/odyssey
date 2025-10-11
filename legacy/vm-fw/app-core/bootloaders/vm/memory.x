MEMORY
{
  /* NOTE 1 K = 1 KiBi = 1024 bytes */
  /* The nRF52840 has 1M of flash */
  FLASH                             : ORIGIN = 0x00000000, LENGTH = 24K
  BOOTLOADER_STATE                  : ORIGIN = 0x00006000, LENGTH = 4K
  ACTIVE                            : ORIGIN = 0x00007000, LENGTH = 256K
  DFU                               : ORIGIN = 0x00047000, LENGTH = 260K
  SETTINGS                          : ORIGIN = 0x000fa000, LENGTH = 24K
  RAM                         (rwx) : ORIGIN = 0x20000000, LENGTH = 256K
}

__bootloader_state_start = ORIGIN(BOOTLOADER_STATE);
__bootloader_state_end = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE);

__bootloader_active_start = ORIGIN(ACTIVE);
__bootloader_active_end = ORIGIN(ACTIVE) + LENGTH(ACTIVE);

__bootloader_dfu_start = ORIGIN(DFU);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU);

ASSERT(ORIGIN(ACTIVE) + LENGTH(ACTIVE) <= ORIGIN(DFU), "
ERROR(bootloader): the ACTIVE and DFU regions are overlapping");
