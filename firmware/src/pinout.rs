#[cfg(feature = "vm2")]
macro_rules! pinout {
    ($p:ident . wf_spim) => ($p.SPI3);
    ($p:ident . wf_spis) => ($p.SPI2);
    ($p:ident . wf_sck) => ($p.P1_00);
    ($p:ident . wf_miso) => ($p.P0_22);
    ($p:ident . wf_mosi) => ($p.P1_07);
    ($p:ident . wf_cs) => ($p.P0_06);
    ($p:ident . wf_fod) => ($p.P1_13);

    ($p:ident . nf_spim) => ($p.TWISPI1);
    ($p:ident . nf_spis) => ($p.TWISPI0);
    ($p:ident . nf_sck) => ($p.P0_05);
    ($p:ident . nf_miso) => ($p.P0_04);
    ($p:ident . nf_mosi) => ($p.P0_26);
    ($p:ident . nf_cs) => ($p.P0_08);
    ($p:ident . nf_fod) => ($p.P0_12);
}

#[cfg(feature = "atslite-1-1")]
macro_rules! pinout {
    ($p:ident . wf_spim) => ($p.SERIAL0);
    ($p:ident . wf_spis) => ($p.SERIAL2);
    ($p:ident . wf_sck) => ($p.P0_20);
    ($p:ident . wf_miso) => ($p.P0_22);
    ($p:ident . wf_mosi) => ($p.P1_01);
    ($p:ident . wf_cs) => ($p.P1_04);
    ($p:ident . wf_fod) => ($p.P0_24);

    ($p:ident . nf_spim) => ($p.SERIAL1);
    ($p:ident . nf_spis) => ($p.SERIAL3);
    ($p:ident . nf_sck) => ($p.P1_08);
    ($p:ident . nf_miso) => ($p.P1_09);
    ($p:ident . nf_mosi) => ($p.P0_29);
    ($p:ident . nf_cs) => ($p.P1_13);
    ($p:ident . nf_fod) => ($p.P1_14);
}
