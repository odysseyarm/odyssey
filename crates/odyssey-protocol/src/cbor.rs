pub(crate) mod heapless_str32 {
    use minicbor::decode::Error;
    use minicbor::encode::{Error as EncodeError, Write};
    use minicbor::{Decoder, Encoder};

    use crate::DeviceName;

    pub(crate) fn encode<Ctx, W: Write>(
        s: &DeviceName,
        e: &mut Encoder<W>,
        _ctx: &mut Ctx,
    ) -> Result<(), EncodeError<W::Error>> {
        e.str(s.as_str())?;
        Ok(())
    }

    pub(crate) fn decode<'b, Ctx>(
        d: &mut Decoder<'b>,
        _ctx: &mut Ctx,
    ) -> Result<DeviceName, Error> {
        Ok(DeviceName::from_str_lossy(d.str()?))
    }

    pub(crate) fn cbor_len<Ctx>(s: &DeviceName, _ctx: &mut Ctx) -> usize {
        let n = s.as_str().len();
        let header = if n <= 23 { 1 } else { 2 };
        header + n
    }
}

pub(crate) mod serde_with {
    use minicbor::{Decoder, Encoder};
    use minicbor_serde::{Deserializer, Serializer};
    use serde::Serialize;

    pub(crate) fn encode<W, C, T>(
        v: &T,
        e: &mut Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>>
    where
        W: minicbor::encode::Write,
        T: Serialize,
    {
        let mut ser = Serializer::new(e.writer_mut());
        v.serialize(&mut ser)
            .map_err(|_| minicbor::encode::Error::message("serde encode error"))
    }

    pub(crate) fn decode<'b, C, T>(
        d: &mut Decoder<'b>,
        _ctx: &mut C,
    ) -> Result<T, minicbor::decode::Error>
    where
        T: serde::de::DeserializeOwned, // owns its data; no borrows from input
    {
        let start = d.position();
        let rest = &d.input()[start..];

        let mut de = Deserializer::new(rest);
        let v: T = T::deserialize(&mut de)
            .map_err(|_| minicbor::decode::Error::message("serde decode error"))?;

        let consumed = de.decoder().position();
        d.set_position(start + consumed);
        Ok(v)
    }

    pub(crate) fn cbor_len<C, T>(v: &T, _ctx: &mut C) -> usize
    where
        T: Serialize,
    {
        struct CountingWriter(usize);

        impl minicbor::encode::Write for CountingWriter {
            type Error = core::convert::Infallible;

            fn write_all(&mut self, buf: &[u8]) -> Result<(), Self::Error> {
                self.0 += buf.len();
                Ok(())
            }
        }

        let mut writer = CountingWriter(0);
        {
            let mut encoder = Encoder::new(&mut writer);
            let mut ser = Serializer::new(encoder.writer_mut());
            v.serialize(&mut ser).unwrap_or(());
        }
        writer.0
    }
}
