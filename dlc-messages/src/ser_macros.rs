//! Set of macro to help implementing the [`lightning::util::ser::Writeable`] trait.
//! All expanded paths are rooted at `$crate`, so callers do not need `lightning` in scope.

/// Writes a field to a writer.
#[macro_export]
macro_rules! field_write {
    ($stream: expr, $field: expr, writeable) => {
        $crate::lightning::util::ser::Writeable::write(&$field, $stream)?;
    };
    ($stream: expr, $field: expr, {cb_writeable, $w_cb: expr, $r_cb: expr}) => {
        $w_cb(&$field, $stream)?;
    };
    ($stream: expr, $field: expr, string) => {
        $crate::ser_impls::write_string(&$field, $stream)?;
    };
    ($stream: expr, $field: expr, vec) => {
        $crate::ser_impls::write_vec(&$field, $stream)?;
    };
    ($stream: expr, $field: expr, {vec_cb, $w_cb: expr, $r_cb: expr}) => {
        $crate::ser_impls::write_vec_cb(&$field, $stream, &$w_cb)?;
    };
    ($stream: expr, $field: expr, {vec_u16_cb, $w_cb: expr, $r_cb: expr}) => {
        $crate::ser_impls::write_vec_u16_cb(&$field, $stream, &$w_cb)?;
    };
    ($stream: expr, $field: expr, float) => {
        $crate::ser_impls::write_f64($field, $stream)?;
    };
    ($stream: expr, $field: expr, usize) => {
        $crate::ser_impls::write_usize(&$field, $stream)?;
    };
    ($stream: expr, $field: expr, SignedAmount) => {
        $crate::ser_impls::write_signed_amount(&$field, $stream)?;
    };
    ($stream: expr, $field: expr, {option_cb, $w_cb: expr, $r_cb: expr}) => {
        $crate::ser_impls::write_option_cb(&$field, $stream, &$w_cb)?;
    };
    ($stream: expr, $field: expr, option) => {
        $crate::ser_impls::write_option(&$field, $stream)?;
    };
}

/// Reads a field from a reader.
#[macro_export]
macro_rules! field_read {
    ($stream: expr, writeable) => {
        $crate::lightning::util::ser::Readable::read($stream)?
    };
    ($stream: expr, {cb_writeable, $w_cb: expr, $r_cb: expr}) => {
        $r_cb($stream)?
    };
    ($stream: expr, string) => {
        $crate::ser_impls::read_string($stream)?
    };
    ($stream: expr, vec) => {
        $crate::ser_impls::read_vec($stream)?
    };
    ($stream: expr, {vec_cb, $w_cb: expr, $r_cb: expr}) => {
        $crate::ser_impls::read_vec_cb($stream, &$r_cb)?
    };
    ($stream: expr, {vec_u16_cb, $w_cb: expr, $r_cb: expr}) => {
        $crate::ser_impls::read_vec_u16_cb($stream, &$r_cb)?
    };
    ($stream: expr, float) => {
        $crate::ser_impls::read_f64($stream)?
    };
    ($stream: expr, usize) => {
        $crate::ser_impls::read_usize($stream)?
    };
    ($stream: expr, SignedAmount) => {
        $crate::ser_impls::read_signed_amount($stream)?
    };
    ($stream: expr, {option_cb, $w_cb: expr, $r_cb: expr}) => {
        $crate::ser_impls::read_option_cb($stream, &$r_cb)?
    };
    ($stream: expr, option) => {
        $crate::ser_impls::read_option($stream)?
    };
}

/// Implements the [`lightning::util::ser::Writeable`] trait for a struct available
/// in this crate.
///
/// A trailing `, $tlv_field` writes a [`TlvStream`](crate::tlv_stream::TlvStream) after the
/// last fixed field and reads it back to the end of the message. It is a separate argument
/// rather than a field kind because the stream must be written last.
#[macro_export]
macro_rules! impl_dlc_writeable {
    ($st:ident, {$(($field: ident, $fieldty: tt)), *} ) => {
        impl $crate::lightning::util::ser::Writeable for $st {
            fn write<W: $crate::lightning::util::ser::Writer>(
                &self,
                w: &mut W,
            ) -> Result<(), $crate::lightning::io::Error> {
                $(
                    $crate::field_write!(w, self.$field, $fieldty);
                )*
                Ok(())
            }
        }

        impl $crate::lightning::util::ser::Readable for $st {
            fn read<R: $crate::lightning::io::Read>(
                r: &mut R,
            ) -> Result<Self, $crate::lightning::ln::msgs::DecodeError> {
                Ok(Self {
                    $(
                        $field: $crate::field_read!(r, $fieldty),
                    )*
                })
            }
        }
    };
    // Version with type_id - writes/reads type_id as first field
    ($st:ident, $type_const:ident, {$(($field: ident, $fieldty: tt)), *} ) => {
        impl $crate::lightning::util::ser::Writeable for $st {
            fn write<W: $crate::lightning::util::ser::Writer>(
                &self,
                w: &mut W,
            ) -> Result<(), $crate::lightning::io::Error> {
                // Write type_id first
                $crate::lightning::util::ser::Writeable::write(&$type_const, w)?;
                $(
                    $crate::field_write!(w, self.$field, $fieldty);
                )*
                Ok(())
            }
        }

        impl $crate::lightning::util::ser::Readable for $st {
            fn read<R: $crate::lightning::io::Read>(
                r: &mut R,
            ) -> Result<Self, $crate::lightning::ln::msgs::DecodeError> {
                // Read and verify type_id first
                let type_id: u16 = $crate::lightning::util::ser::Readable::read(r)?;
                if type_id != $type_const {
                    return Err($crate::lightning::ln::msgs::DecodeError::UnknownRequiredFeature);
                }
                Ok(Self {
                    $(
                        $field: $crate::field_read!(r, $fieldty),
                    )*
                })
            }
        }
    };
    // Version with type_id and a trailing TLV stream.
    ($st:ident, $type_const:ident, {$(($field: ident, $fieldty: tt)), *}, $tlv_field: ident ) => {
        impl $crate::lightning::util::ser::Writeable for $st {
            fn write<W: $crate::lightning::util::ser::Writer>(
                &self,
                w: &mut W,
            ) -> Result<(), $crate::lightning::io::Error> {
                // Write type_id first
                $crate::lightning::util::ser::Writeable::write(&$type_const, w)?;
                $(
                    $crate::field_write!(w, self.$field, $fieldty);
                )*
                // Last, always: the reader takes everything after this point as the stream.
                $crate::lightning::util::ser::Writeable::write(&self.$tlv_field, w)?;
                Ok(())
            }
        }

        impl $crate::lightning::util::ser::Readable for $st {
            fn read<R: $crate::lightning::io::Read>(
                r: &mut R,
            ) -> Result<Self, $crate::lightning::ln::msgs::DecodeError> {
                // Read and verify type_id first
                let type_id: u16 = $crate::lightning::util::ser::Readable::read(r)?;
                if type_id != $type_const {
                    return Err($crate::lightning::ln::msgs::DecodeError::UnknownRequiredFeature);
                }
                Ok(Self {
                    $(
                        $field: $crate::field_read!(r, $fieldty),
                    )*
                    $tlv_field: $crate::tlv_stream::TlvStream::read_to_end(r)?,
                })
            }
        }

        impl $st {
            /// Reads the fixed fields after the type prefix, leaving the TLV
            /// stream empty. For parsing data serialized before the message
            /// carried a stream, where other fields follow the message bytes
            /// and the caller has already consumed the type.
            pub fn read_body_without_tlv_stream<R: $crate::lightning::io::Read>(
                r: &mut R,
            ) -> Result<Self, $crate::lightning::ln::msgs::DecodeError> {
                Ok(Self {
                    $(
                        $field: $crate::field_read!(r, $fieldty),
                    )*
                    $tlv_field: $crate::tlv_stream::TlvStream::default(),
                })
            }
        }
    };
}

/// Implements the [`lightning::util::ser::Writeable`] trait for a struct external
/// to this crate.
#[macro_export]
macro_rules! impl_dlc_writeable_external {
    ($st: ident $(< $gen: ident $(< $gen2: ident >)?> )? , $name: ident, {$(($field: ident, $fieldty: tt)), *} ) => {
        /// Module containing write and read functions for $name
        pub mod $name {
            use super::*;
            /// Function to write $name
            pub fn write<W: $crate::lightning::util::ser::Writer>(
                $name: &$st<$($gen$(<$gen2>)?)?>,
                w: &mut W,
            ) -> Result<(), $crate::lightning::io::Error> {
                $(
                    $crate::field_write!(w, $name.$field, $fieldty);
                )*
                Ok(())
            }

            /// Function to read $name
            pub fn read<R: $crate::lightning::io::Read>(
                r: &mut R,
            ) -> Result<$st<$($gen$(<$gen2>)?)?>, $crate::lightning::ln::msgs::DecodeError> {
                Ok($st {
                    $(
                        $field: $crate::field_read!(r, $fieldty),
                    )*
                })
            }
        }
    };
}

/// Implements the [`lightning::util::ser::Writeable`] trait for an enum external
/// to this crate.
#[macro_export]
macro_rules! impl_dlc_writeable_external_enum {
    ($st:ident $(<$gen: ident>)?, $name: ident, $(($variant_id: expr, $variant_name: ident, $variant_mod: ident)), * ) => {
        mod $name {
            use super::*;

            pub fn write<W: $crate::lightning::util::ser::Writer>(
                $name: &$st$(<$gen>)?,
                w: &mut W,
            ) -> Result<(), $crate::lightning::io::Error> {
                match $name {
                    $($st::$variant_name(ref field) => {
                        let id : u8 = $variant_id;
                        $crate::lightning::util::ser::Writeable::write(&id, w)?;
                        $variant_mod::write(field, w)?;
                    }),*
                };
                Ok(())
            }

            pub fn read<R: $crate::lightning::io::Read>(
                r: &mut R,
            ) -> Result<$st$(<$gen>)?, $crate::lightning::ln::msgs::DecodeError> {
                let id: u8 = $crate::lightning::util::ser::Readable::read(r)?;
                match id {
                    $($variant_id => {
                        Ok($st::$variant_name($variant_mod::read(r)?))
                    }),*
                    _ => {
                        Err($crate::lightning::ln::msgs::DecodeError::UnknownRequiredFeature)
                    },
                }
            }
        }
    };
}

/// Declares a type as a TLV record, giving it its record type in one place.
///
/// Use this for any type the DLC specification assigns a TLV record type to, and for any
/// record an application defines for itself. It implements
/// [`TlvType`](crate::ser_impls::TlvType) with the given constant and derives
/// [`Type`](lightning::ln::wire::Type) from it, so the two can never disagree. The type
/// must be `Debug`, which is what `Type` requires.
///
/// Declaring it also brings in [`TlvRecord`](crate::ser_impls::TlvRecord) through that
/// trait's blanket impl, which is what lets the type be read and written on its own
/// as well as nested. A type that only ever appears nested loses nothing by having it.
///
/// ```ignore
/// impl_dlc_tlv_record!(OracleAnnouncement, ANNOUNCEMENT_TYPE);
/// ```
///
/// An application picking its own record type should take an odd one in the custom range,
/// where the DLC specification assigns nothing, so a peer that does not know the record
/// carries it through instead of rejecting the message.
///
/// Do not reach for this to mark a peer-to-peer protocol message. Those are wire
/// messages — a `u16` type and no length — and want a bare
/// [`Type`](lightning::ln::wire::Type) impl instead.
#[macro_export]
macro_rules! impl_dlc_tlv_record {
    ($st:ident, $type_id:expr) => {
        impl $crate::ser_impls::TlvType for $st {
            const TYPE_ID: u16 = $type_id;
        }

        impl $crate::lightning::ln::wire::Type for $st {
            fn type_id(&self) -> u16 {
                <$st as $crate::ser_impls::TlvType>::TYPE_ID
            }
        }
    };
}

/// Implements the [`lightning::util::ser::Writeable`] trait for an enum as a TLV.
#[macro_export]
macro_rules! impl_dlc_writeable_enum_as_tlv {
    ($st:ident, $(($variant_id: expr, $variant_name: ident)), *;) => {
        impl $crate::lightning::util::ser::Writeable for $st {
            fn write<W: $crate::lightning::util::ser::Writer>(
                &self,
                w: &mut W,
            ) -> Result<(), $crate::lightning::io::Error> {
                match self {
                    $($st::$variant_name(ref field) => {
                        $crate::lightning::util::ser::Writeable::write(
                            &$crate::ser_impls::BigSize($variant_id as u64), w)?;
                        $crate::lightning::util::ser::Writeable::write(
                            &$crate::ser_impls::BigSize(
                                $crate::lightning::util::ser::Writeable::serialized_length(field)
                                    as u64,
                            ),
                            w,
                        )?;
                        $crate::lightning::util::ser::Writeable::write(field, w)?;
                    }),*
                };
                Ok(())
            }
        }

        impl $crate::lightning::util::ser::Readable for $st {
            fn read<R: $crate::lightning::io::Read>(
                r: &mut R,
            ) -> Result<Self, $crate::lightning::ln::msgs::DecodeError> {
                let id: $crate::ser_impls::BigSize =
                    $crate::lightning::util::ser::Readable::read(r)?;
                match id.0 {
                    $($variant_id => {
                        let len : $crate::ser_impls::BigSize =
                            $crate::lightning::util::ser::Readable::read(r)?;
                        Ok($st::$variant_name($crate::ser_impls::read_tlv_body(r, len.0)?))
                    }),*
                    _ => {
                        Err($crate::lightning::ln::msgs::DecodeError::UnknownRequiredFeature)
                    },
                }
            }
        }
    };
}

/// Implements the [`lightning::util::ser::Writeable`] trait for an enum.
#[macro_export]
macro_rules! impl_dlc_writeable_enum {
    ($st:ident, $(($tuple_variant_id: expr, $tuple_variant_name: ident)), *;
    $(($variant_id: expr, $variant_name: ident, {$(($field: ident, $fieldty: tt)),*})), *;
    $(($external_variant_id: expr, $external_variant_name: ident, $write_cb: expr, $read_cb: expr)), *;
    $(($simple_variant_id: expr, $simple_variant_name: ident)), *) => {
        impl $crate::lightning::util::ser::Writeable for $st {
            fn write<W: $crate::lightning::util::ser::Writer>(
                &self,
                w: &mut W,
            ) -> Result<(), $crate::lightning::io::Error> {
                match self {
                    $($st::$tuple_variant_name(ref field) => {
                        let id : u8 = $tuple_variant_id;
                        $crate::lightning::util::ser::Writeable::write(&id, w)?;
                        $crate::lightning::util::ser::Writeable::write(field, w)?;
                    }),*
                    $($st::$variant_name { $(ref $field),* } => {
                        let id : u8 = $variant_id;
                        $crate::lightning::util::ser::Writeable::write(&id, w)?;
                        $(
                            $crate::field_write!(w, $field, $fieldty);
                        )*
                    }),*
                    $($st::$external_variant_name(ref field) => {
                        let id : u8 = $external_variant_id;
                        $crate::lightning::util::ser::Writeable::write(&id, w)?;
                        $write_cb(field, w)?;
                    }),*
                    $($st::$simple_variant_name => {
                        let id : u8 = $simple_variant_id;
                        $crate::lightning::util::ser::Writeable::write(&id, w)?;
                    }),*
                };
                Ok(())
            }
        }

        impl $crate::lightning::util::ser::Readable for $st {
            fn read<R: $crate::lightning::io::Read>(
                r: &mut R,
            ) -> Result<Self, $crate::lightning::ln::msgs::DecodeError> {
                let id: u8 = $crate::lightning::util::ser::Readable::read(r)?;
                match id {
                    $($tuple_variant_id => {
                        Ok($st::$tuple_variant_name(
                            $crate::lightning::util::ser::Readable::read(r)?))
                    }),*
                    $($variant_id => {
                        Ok($st::$variant_name {
                            $(
                                $field: $crate::field_read!(r, $fieldty)
                            ),*
                        })
                    }),*
                    $($external_variant_id => {
                        Ok($st::$external_variant_name($read_cb(r)?))
                    }),*
                    $($simple_variant_id => {
                        Ok($st::$simple_variant_name)
                    }),*
                    _ => {
                        Err($crate::lightning::ln::msgs::DecodeError::UnknownRequiredFeature)
                    },
                }
            }
        }
    };
}
