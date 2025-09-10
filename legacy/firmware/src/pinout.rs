#[cfg(feature = "vm2")]
macro_rules! pinout {
    ($p:ident . wf_spim) => ($p.SPI3);
    ($p:ident . wf_spis) => ($p.SPI2);
    ($p:ident . wf_sck) => ($p.P1_00);
    ($p:ident . wf_mosi) => ($p.P1_07);
    ($p:ident . wf_miso) => ($p.P0_22);
    ($p:ident . wf_cs) => ($p.P0_06);
    ($p:ident . wf_fod) => ($p.P1_13);

    ($p:ident . nf_spim) => ($p.TWISPI1);
    ($p:ident . nf_spis) => ($p.TWISPI0);
    ($p:ident . nf_sck) => ($p.P0_05);
    ($p:ident . nf_mosi) => ($p.P0_26);
    ($p:ident . nf_miso) => ($p.P0_04);
    ($p:ident . nf_cs) => ($p.P0_08);
    ($p:ident . nf_fod) => ($p.P0_12);
}

#[cfg(feature = "atslite-1-1")]
macro_rules! pinout {
    ($p:ident . wf_spim) => ($p.SERIAL0);
    ($p:ident . wf_spis) => ($p.SERIAL2);
    ($p:ident . wf_sck) => ($p.P0_20);
    ($p:ident . wf_mosi) => ($p.P1_01);
    ($p:ident . wf_miso) => ($p.P0_22);
    ($p:ident . wf_cs) => ($p.P1_04);
    ($p:ident . wf_fod) => ($p.P0_24);

    ($p:ident . nf_spim) => ($p.SERIAL1);
    ($p:ident . nf_spis) => ($p.SERIAL3);
    ($p:ident . nf_sck) => ($p.P1_08);
    ($p:ident . nf_mosi) => ($p.P0_29);
    ($p:ident . nf_miso) => ($p.P1_09);
    ($p:ident . nf_cs) => ($p.P1_13);
    ($p:ident . nf_fod) => ($p.P1_14);

    ($p:ident . pwr_btn) => ($p.P1_08); // active low
}

#[cfg(feature = "atslite-2-2")]
macro_rules! pinout {
    ($p:ident . wf_spim) => ($p.SERIAL0);
    ($p:ident . wf_spis) => ($p.SERIAL2);
    ($p:ident . wf_sck) => ($p.P1_05);
    ($p:ident . wf_mosi) => ($p.P1_06);
    ($p:ident . wf_miso) => ($p.P0_29);
    ($p:ident . wf_cs) => ($p.P1_04);
    ($p:ident . wf_fod) => ($p.P1_07);

    ($p:ident . nf_spim) => ($p.SERIAL1);
    ($p:ident . nf_spis) => ($p.SERIAL3);
    ($p:ident . nf_sck) => ($p.P1_11);
    ($p:ident . nf_mosi) => ($p.P1_12);
    ($p:ident . nf_miso) => ($p.P1_13);
    ($p:ident . nf_cs) => ($p.P0_30);
    ($p:ident . nf_fod) => ($p.P0_10);

    ($p:ident . pwr_btn) => ($p.P1_08); // active low
}

#[cfg(feature = "atslite-4-1")]
macro_rules! pinout {
    ($p:ident . wf_spim) => ($p.SERIAL0);
    ($p:ident . wf_spis) => ($p.SERIAL2);
    ($p:ident . wf_sck) => ($p.P0_23);
    ($p:ident . wf_mosi) => ($p.P1_08);
    ($p:ident . wf_miso) => ($p.P1_07);
    ($p:ident . wf_cs) => ($p.P1_04);
    ($p:ident . wf_fod) => ($p.P0_10);

    ($p:ident . nf_spim) => ($p.SERIAL1);
    ($p:ident . nf_spis) => ($p.SERIAL3);
    ($p:ident . nf_sck) => ($p.P0_14);
    ($p:ident . nf_mosi) => ($p.P0_16);
    ($p:ident . nf_miso) => ($p.P0_13);
    ($p:ident . nf_cs) => ($p.P0_15);
    ($p:ident . nf_fod) => ($p.P1_05);

    ($p:ident . pwr_btn) => ($p.P1_10); // no button on 4.1
}
