#[macro_export]
macro_rules! define_wrapped_ffi_struct {
    ($name:ident, { $($field:ident : $type:ty),* $(,)? }) => {
        paste::paste! {
            #[repr(C)]
            #[derive(Clone)]
            pub struct $name(pub [<Ffi $name>]);

            #[ffi_type(name = stringify!($name))]
            #[derive(Clone)]
            pub struct [<Ffi $name>] {
                $(pub $field: $type),*
            }

            impl From<[<Ffi $name>]> for $name {
                fn from(inner: [<Ffi $name>]) -> Self {
                    Self(inner)
                }
            }

            impl From<$name> for [<Ffi $name>] {
                fn from(wrapper: $name) -> Self {
                    wrapper.0
                }
            }
        }
    };
}

#[macro_export]
macro_rules! impl_from_simple {
    ($from:path => $to:ty, $($field:ident),+) => {
        impl From<$from> for $to {
            fn from(value: $from) -> Self {
                Self {
                    $(
                        $field: value.$field.into(),
                    )+
                }
            }
        }
    };
}

#[macro_export]
macro_rules! impl_from_wrapped_ffi {
    ($external:path => $wrapper:ident, $($field:ident),+) => {
        impl From<$external> for Ffi$wrapper {
            fn from(value: $external) -> Self {
                Self {
                    $(
                        $field: value.$field.into(),
                    )+
                }
            }
        }

        impl From<Ffi$wrapper> for $external {
            fn from(value: Ffi$wrapper) -> Self {
                Self {
                    $(
                        $field: value.$field.into(),
                    )+
                }
            }
        }

        impl From<$external> for $wrapper {
            fn from(value: $external) -> Self {
                Self(Ffi$wrapper::from(value))
            }
        }

        impl From<$wrapper> for $external {
            fn from(value: $wrapper) -> Self {
                Ffi$wrapper::from(value.0)
            }
        }
    };
}
