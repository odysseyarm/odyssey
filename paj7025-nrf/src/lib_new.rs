pub mod registers {
    device_driver::create_device!(
        device_name: Paj7025,
        dsl: {
            config {
                type DefaultRegisterAccess = RW;
                type DefaultFieldAccess = RW;
                type DefaultBufferAccess = RW;
                type DefaultByteOrder = LE;
                type DefaultBitOrder = LSB0;
                type RegisterAddressType = u16; // [bank][address]
                type NameWordBoundaries = [
                    Underscore, Hyphen, Space, LowerUpper,
                    UpperDigit, DigitUpper, DigitLower,
                    LowerDigit, Acronym,
                ];
                type DefmtFeature = "defmt";
            }
            block Control {
                block Bank0 {
                    register Bank0_Sync_Updated_Flag {
                        type Access = WO;
                        const ADDRESS = 0x01;
                        const SIZE_BITS = 8;
                    },
                    register Product_ID {
                        type Access = RO;
                        const ADDRESS = 0x02;
                        const SIZE_BITS = 16;
                    },
                    #[doc = "DSP settings; area max threshold"]
                    register Cmd_oahb {
                        const ADDRESS = 0x0B;
                        const SIZE_BITS = 14;
                    },
                    #[doc = "DSP settings; noise threshold"]
                    register Cmd_nthd {
                        const ADDRESS = 0x0F;
                        const SIZE_BITS = 8;
                    },
                    register Cmd_dsp_operation_mode {
                        const ADDRESS = 0x12;
                        const SIZE_BITS = 8;
                    },
                    register Cmd_max_object_num {
                        const ADDRESS = 0x19;
                        const SIZE_BITS = 5;
                    },
                    register Cmd_FrameSubtraction_On {
                        const ADDRESS = 0x28;
                        const SIZE_BITS = 8;
                    },
                },
                block Bank1 {
                    const ADDRESS_OFFSET = 0x0100;
                    register Bank1_Sync_Updated_Flag {
                        type Access = WO;
                        const ADDRESS = 0x01;
                        const SIZE_BITS = 1;
                    },
                    #[doc = "Read sensor gain 1"]
                    register B_global_R {
                        type Access = RO;
                        const ADDRESS = 0x05;
                        const SIZE_BITS = 5;
                    },
                    #[doc = "Read sensor gain 2"]
                    register B_ggh_R {
                        type Access = RO;
                        const ADDRESS = 0x06;
                        const SIZE_BITS = 2;
                    },
                    /// Read exposure time
                    /// unit = 200ns
                    /// Note: The minimum setting is 100.
                    register B_expo_R {
                        type Access = RO;
                        const ADDRESS = 0x0E;
                        const SIZE_BITS = 16;
                    },
                },
                block BankC {
                    const ADDRESS_OFFSET = 0x0C00;
                    #[doc = "Frame period; unit = 100ns"]
                    register Cmd_frame_period {
                        const ADDRESS = 0x07;
                        const SIZE_BITS = 24;
                    },
                    /// Write sensor gain 1 (needs Bank1 sync)
                    /// Note: The minimum total gain (gain1+gain2) setting is 2X.
                    register B_global {
                        type Access = WO;
                        const ADDRESS = 0x0B;
                        const SIZE_BITS = 5;
                    },
                    #[doc = "Write sensor gain 2 (needs Bank1 sync)"]
                    register B_ggh {
                        type Access = WO;
                        const ADDRESS = 0x0C;
                        const SIZE_BITS = 2;
                    },
                    #[doc = "Write sensor exposure time (needs Bank1 sync)"]
                    register B_expo {
                        type Access = WO;
                        const ADDRESS = 0x0F;
                        const SIZE_BITS = 16;
                    },
                    #[doc = "DSP settings; area min threshold"]
                    register Cmd_oalb {
                        const ADDRESS = 0x46;
                        const SIZE_BITS = 8;
                    },
                    #[doc = "DSP settings; brightness threshold"]
                    register Cmd_thd {
                        const ADDRESS = 0x47;
                        const SIZE_BITS = 8;
                    },
                    #[doc = "DSP settings; x-axis Interpolated Resolution"]
                    register Cmd_scale_resolution_x {
                        const ADDRESS = 0x60;
                        const SIZE_BITS = 12;
                    },
                    #[doc = "DSP settings; y-axis Interpolated Resolution"]
                    register Cmd_scale_resolution_y {
                        const ADDRESS = 0x62;
                        const SIZE_BITS = 12;
                    }
                },
            }
        }
    );
}
