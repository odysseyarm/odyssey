MEMORY
{
  BOOTLOADER                        : ORIGIN = 0x00000000, LENGTH = 24K
  BOOTLOADER_STATE                  : ORIGIN = 0x00006000, LENGTH = 4K
  FLASH                             : ORIGIN = 0x00007000, LENGTH = 256K
  DFU                               : ORIGIN = 0x00047000, LENGTH = 260K
  SETTINGS                          : ORIGIN = 0x000fa000, LENGTH = 24K
  RAM                         (rwx) : ORIGIN = 0x20000000, LENGTH = 256K
  ICMSG_RX                          : ORIGIN = 0x20070000, LENGTH = 0x0800
  ICMSG_TX                          : ORIGIN = 0x20078000, LENGTH = 0x0800
}

__bootloader_state_start = ORIGIN(BOOTLOADER_STATE);
__bootloader_state_end = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE);

__bootloader_dfu_start = ORIGIN(DFU);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU);

__settings_start = ORIGIN(SETTINGS);
__settings_end = ORIGIN(SETTINGS) + LENGTH(SETTINGS);

__icmsg_tx_start = ORIGIN(ICMSG_TX);
__icmsg_tx_end = ORIGIN(ICMSG_TX) + LENGTH(ICMSG_TX);
__icmsg_rx_start = ORIGIN(ICMSG_RX);
__icmsg_rx_end = ORIGIN(ICMSG_RX) + LENGTH(ICMSG_RX);

ASSERT(ORIGIN(FLASH) + LENGTH(FLASH) <= ORIGIN(DFU), "
ERROR(bootloader): the FLASH and DFU regions are overlapping");

ASSERT(ORIGIN(RAM) + LENGTH(RAM) <= ORIGIN(ICMSG_RX), "
ERROR: the RAM and ICMSG_RX regions are overlapping");
ASSERT(ORIGIN(ICMSG_RX) + LENGTH(ICMSG_RX) <= ORIGIN(ICMSG_TX), "
ERROR: the ICMSG_TX and ICMSG_RX regions are overlapping");
