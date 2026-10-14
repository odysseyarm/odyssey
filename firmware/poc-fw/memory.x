MEMORY
{
  /* NOTE 1 K = 1 KiBi = 1024 bytes */
  /* The nRF5340 has 1M of flash */
  BOOTLOADER                        : ORIGIN = 0x00000000, LENGTH = 24K
  BOOTLOADER_STATE                  : ORIGIN = 0x00006000, LENGTH = 4K
  FLASH                             : ORIGIN = 0x00007000, LENGTH = 256K
  DFU                               : ORIGIN = 0x00047000, LENGTH = 260K
  SETTINGS                          : ORIGIN = 0x000fa000, LENGTH = 24K
  RAM                         (rwx) : ORIGIN = 0x20000000, LENGTH = 256K
}

__bootloader_state_start = ORIGIN(BOOTLOADER_STATE);
__bootloader_state_end = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE);

__bootloader_dfu_start = ORIGIN(DFU);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU);

__settings_start = ORIGIN(SETTINGS);
__settings_end = ORIGIN(SETTINGS) + LENGTH(SETTINGS);

ASSERT(ORIGIN(FLASH) + LENGTH(FLASH) <= ORIGIN(DFU), "
ERROR(bootloader): the FLASH and DFU regions are overlapping");
