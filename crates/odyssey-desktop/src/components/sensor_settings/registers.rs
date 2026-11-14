use odyssey_hub_client::client::DeviceClient;

/// Sensor ports on the device: the near-field sensor and the wide-field one.
pub(super) const NEAR_FIELD_PORT: u8 = 0;
pub(super) const WIDE_FIELD_PORT: u8 = 1;

/// One byte-wide sensor register.
#[derive(Clone, Copy)]
pub(super) struct Register {
    bank: u8,
    addr: u8,
}

impl Register {
    pub(super) const fn new(bank: u8, addr: u8) -> Self {
        Self { bank, addr }
    }
}

/// A multi-byte value spread over `N` consecutive registers of one bank, low
/// byte first.
#[derive(Clone, Copy)]
pub(super) struct RegisterRun<const N: usize> {
    bank: u8,
    addrs: [u8; N],
}

impl<const N: usize> RegisterRun<N> {
    pub(super) const fn new(bank: u8, addrs: [u8; N]) -> Self {
        Self { bank, addrs }
    }

    fn registers(self) -> [Register; N] {
        self.addrs.map(|addr| Register::new(self.bank, addr))
    }
}

/// Register access to one sensor port of one device, through the hub.
///
/// Multi-byte values go one RPC per byte, low byte first.
pub(super) struct Registers<'a> {
    client: &'a DeviceClient,
    port: u8,
}

impl<'a> Registers<'a> {
    pub(super) fn new(client: &'a DeviceClient, port: u8) -> Self {
        Self { client, port }
    }

    pub(super) async fn read(&self, register: Register) -> eros::Result<u8> {
        let value = self
            .client
            .read_register(self.port, register.bank, register.addr)
            .await?;
        Ok(value)
    }

    pub(super) async fn write(&self, register: Register, value: u8) -> eros::Result<()> {
        self.client
            .write_register(self.port, register.bank, register.addr, value)
            .await?;
        Ok(())
    }

    pub(super) async fn read_u16(&self, run: RegisterRun<2>) -> eros::Result<u16> {
        Ok(u16::from_le_bytes(self.read_bytes(run).await?))
    }

    pub(super) async fn write_u16(&self, run: RegisterRun<2>, value: u16) -> eros::Result<()> {
        self.write_bytes(run, value.to_le_bytes()).await
    }

    /// A 24-bit value held in three registers.
    pub(super) async fn read_u24(&self, run: RegisterRun<3>) -> eros::Result<u32> {
        let [b0, b1, b2] = self.read_bytes(run).await?;
        Ok(u32::from_le_bytes([b0, b1, b2, 0]))
    }

    /// The top byte of `value` is dropped. Callers validate it fits in 24 bits.
    pub(super) async fn write_u24(&self, run: RegisterRun<3>, value: u32) -> eros::Result<()> {
        let [b0, b1, b2, _] = value.to_le_bytes();
        self.write_bytes(run, [b0, b1, b2]).await
    }

    async fn read_bytes<const N: usize>(&self, run: RegisterRun<N>) -> eros::Result<[u8; N]> {
        let mut bytes = [0; N];
        for (byte, register) in bytes.iter_mut().zip(run.registers()) {
            *byte = self.read(register).await?;
        }
        Ok(bytes)
    }

    async fn write_bytes<const N: usize>(
        &self,
        run: RegisterRun<N>,
        bytes: [u8; N],
    ) -> eros::Result<()> {
        for (register, byte) in run.registers().into_iter().zip(bytes) {
            self.write(register, byte).await?;
        }
        Ok(())
    }
}
