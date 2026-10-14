MEMORY
{
  /* Network core bootloader region (overridden by build.rs memory.x in OUT_DIR) */
  FLASH : ORIGIN = 0x01000000, LENGTH = 192K
  /* Network core RAM */
  RAM : ORIGIN = 0x21000000, LENGTH = 64K
}
