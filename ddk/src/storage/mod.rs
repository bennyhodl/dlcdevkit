pub mod memory;
#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "sled")]
pub mod sled;

#[cfg(feature = "postgres")]
pub mod sqlx;

/// The storage key of a BIP-329 label: the record type tag plus the
/// reference, because an input record and an output record can share the
/// same outpoint reference.
pub fn label_key(label_ref: &bip329::LabelRef) -> String {
    use bip329::LabelRef;
    match label_ref {
        LabelRef::Txid(txid) => format!("tx:{txid}"),
        LabelRef::Address(address) => format!("addr:{}", address.clone().assume_checked()),
        LabelRef::PublicKey(pubkey) => format!("pubkey:{pubkey}"),
        LabelRef::Input(outpoint) => format!("input:{outpoint}"),
        LabelRef::Output(outpoint) => format!("output:{outpoint}"),
        LabelRef::Xpub(xpub) => format!("xpub:{xpub}"),
    }
}
