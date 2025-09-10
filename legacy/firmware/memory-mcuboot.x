MEMORY
{
  /* slot0 of mcuboot for vision modules */
  FLASH : ORIGIN = 0xC000 + 512, LENGTH = 0x67000 - 512
  /* FLASH   : ORIGIN = 0, LENGTH = 1024K */
  /* CODERAM : ORIGIN = 0x00800000, LENGTH = 64K */  /* atslite doesn't have code ram :'( */
  CODERAM : ORIGIN = 0x20000000 + 8K, LENGTH = 64K  /* mcuboot locks the first page of memory to read only on the atslite */
  RAM     : ORIGIN = 0x20000000 + 64K + 8K, LENGTH = 256K - 64K - 8K
}
