use std::{
    fmt,
    str::{self, FromStr},
};

use elements::{self, Script};

use super::{checksum::verify_checksum, Bare, ElementsTrait, Pkh, Sh, Wpkh, Wsh};
use crate::{expression, DescriptorTrait, Error, MiniscriptKey, Satisfier, ToPublicKey};

/// Script descriptor
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PreTaprootDescriptor<Pk: MiniscriptKey> {
    /// Bare descriptor
    Bare(Bare<Pk>),
    /// Pay-to-PubKey-Hash
    Pkh(Pkh<Pk>),
    /// Pay-to-Witness-PubKey-Hash
    Wpkh(Wpkh<Pk>),
    /// Pay-to-ScriptHash(includes nested wsh/wpkh/sorted multi)
    Sh(Sh<Pk>),
    /// Pay-to-Witness-ScriptHash with Segwitv0 context
    Wsh(Wsh<Pk>),
}

impl<Pk: MiniscriptKey> ElementsTrait<Pk> for PreTaprootDescriptor<Pk> {
    fn blind_addr(
        &self,
        blinder: Option<elements::secp256k1_zkp::PublicKey>,
        params: &'static elements::AddressParams,
    ) -> Result<elements::Address, Error>
    where
        Pk: ToPublicKey,
    {
        match self {
            PreTaprootDescriptor::Bare(bare) => bare.blind_addr(blinder, params),
            PreTaprootDescriptor::Pkh(pkh) => pkh.blind_addr(blinder, params),
            PreTaprootDescriptor::Wpkh(wpkh) => wpkh.blind_addr(blinder, params),
            PreTaprootDescriptor::Sh(sh) => sh.blind_addr(blinder, params),
            PreTaprootDescriptor::Wsh(wsh) => wsh.blind_addr(blinder, params),
        }
    }
}

impl<Pk: MiniscriptKey> DescriptorTrait<Pk> for PreTaprootDescriptor<Pk> {
    /// Whether the descriptor is safe
    /// Checks whether all the spend paths in the descriptor are possible
    /// on the bitcoin network under the current standardness and consensus rules
    /// Also checks whether the descriptor requires signauture on all spend paths
    /// And whether the script is malleable.
    /// In general, all the guarantees of miniscript hold only for safe scripts.
    /// All the analysis guarantees of miniscript only hold safe scripts.
    /// The signer may not be able to find satisfactions even if one exists
    fn sanity_check(&self) -> Result<(), Error> {
        match *self {
            PreTaprootDescriptor::Bare(ref bare) => bare.sanity_check(),
            PreTaprootDescriptor::Pkh(ref pkh) => pkh.sanity_check(),
            PreTaprootDescriptor::Wpkh(ref wpkh) => wpkh.sanity_check(),
            PreTaprootDescriptor::Wsh(ref wsh) => wsh.sanity_check(),
            PreTaprootDescriptor::Sh(ref sh) => sh.sanity_check(),
        }
    }

    /// Computes the scriptpubkey of the descriptor
    fn script_pubkey(&self) -> Script
    where
        Pk: ToPublicKey,
    {
        match *self {
            PreTaprootDescriptor::Bare(ref bare) => bare.script_pubkey(),
            PreTaprootDescriptor::Pkh(ref pkh) => pkh.script_pubkey(),
            PreTaprootDescriptor::Wpkh(ref wpkh) => wpkh.script_pubkey(),
            PreTaprootDescriptor::Wsh(ref wsh) => wsh.script_pubkey(),
            PreTaprootDescriptor::Sh(ref sh) => sh.script_pubkey(),
        }
    }

    /// Computes the scriptSig that will be in place for an unsigned
    /// input spending an output with this descriptor. For pre-segwit
    /// descriptors, which use the scriptSig for signatures, this
    /// returns the empty script.
    ///
    /// This is used in Segwit transactions to produce an unsigned
    /// transaction whose txid will not change during signing (since
    /// only the witness data will change).
    fn unsigned_script_sig(&self) -> Script
    where
        Pk: ToPublicKey,
    {
        match *self {
            PreTaprootDescriptor::Bare(ref bare) => bare.unsigned_script_sig(),
            PreTaprootDescriptor::Pkh(ref pkh) => pkh.unsigned_script_sig(),
            PreTaprootDescriptor::Wpkh(ref wpkh) => wpkh.unsigned_script_sig(),
            PreTaprootDescriptor::Wsh(ref wsh) => wsh.unsigned_script_sig(),
            PreTaprootDescriptor::Sh(ref sh) => sh.unsigned_script_sig(),
        }
    }

    /// Computes the "witness script" of the descriptor, i.e. the underlying
    /// script before any hashing is done. For `Bare`, `Pkh` and `Wpkh` this
    /// is the scriptPubkey; for `ShWpkh` and `Sh` this is the redeemScript;
    /// for the others it is the witness script.
    /// Errors:
    /// - When the descriptor is Tr
    fn explicit_script(&self) -> Result<Script, Error>
    where
        Pk: ToPublicKey,
    {
        match *self {
            PreTaprootDescriptor::Bare(ref bare) => bare.explicit_script(),
            PreTaprootDescriptor::Pkh(ref pkh) => pkh.explicit_script(),
            PreTaprootDescriptor::Wpkh(ref wpkh) => wpkh.explicit_script(),
            PreTaprootDescriptor::Wsh(ref wsh) => wsh.explicit_script(),
            PreTaprootDescriptor::Sh(ref sh) => sh.explicit_script(),
        }
    }

    /// Returns satisfying non-malleable witness and scriptSig to spend an
    /// output controlled by the given descriptor if it possible to
    /// construct one using the satisfier S.
    fn get_satisfaction<S>(&self, satisfier: S) -> Result<(Vec<Vec<u8>>, Script), Error>
    where
        Pk: ToPublicKey,
        S: Satisfier<Pk>,
    {
        match *self {
            PreTaprootDescriptor::Bare(ref bare) => bare.get_satisfaction(satisfier),
            PreTaprootDescriptor::Pkh(ref pkh) => pkh.get_satisfaction(satisfier),
            PreTaprootDescriptor::Wpkh(ref wpkh) => wpkh.get_satisfaction(satisfier),
            PreTaprootDescriptor::Wsh(ref wsh) => wsh.get_satisfaction(satisfier),
            PreTaprootDescriptor::Sh(ref sh) => sh.get_satisfaction(satisfier),
        }
    }

    /// Returns a possilbly mallable satisfying non-malleable witness and scriptSig to spend an
    /// output controlled by the given descriptor if it possible to
    /// construct one using the satisfier S.
    fn get_satisfaction_mall<S>(&self, satisfier: S) -> Result<(Vec<Vec<u8>>, Script), Error>
    where
        Pk: ToPublicKey,
        S: Satisfier<Pk>,
    {
        match *self {
            PreTaprootDescriptor::Bare(ref bare) => bare.get_satisfaction_mall(satisfier),
            PreTaprootDescriptor::Pkh(ref pkh) => pkh.get_satisfaction_mall(satisfier),
            PreTaprootDescriptor::Wpkh(ref wpkh) => wpkh.get_satisfaction_mall(satisfier),
            PreTaprootDescriptor::Wsh(ref wsh) => wsh.get_satisfaction_mall(satisfier),
            PreTaprootDescriptor::Sh(ref sh) => sh.get_satisfaction_mall(satisfier),
        }
    }

    /// Computes an upper bound on the weight of a satisfying witness to the
    /// transaction. Assumes all signatures are 73 bytes, including push opcode
    /// and sighash suffix. Includes the weight of the VarInts encoding the
    /// scriptSig and witness stack length.
    fn max_satisfaction_weight(&self) -> Result<usize, Error> {
        match *self {
            PreTaprootDescriptor::Bare(ref bare) => bare.max_satisfaction_weight(),
            PreTaprootDescriptor::Pkh(ref pkh) => pkh.max_satisfaction_weight(),
            PreTaprootDescriptor::Wpkh(ref wpkh) => wpkh.max_satisfaction_weight(),
            PreTaprootDescriptor::Wsh(ref wsh) => wsh.max_satisfaction_weight(),
            PreTaprootDescriptor::Sh(ref sh) => sh.max_satisfaction_weight(),
        }
    }

    /// Get the `scriptCode` of a transaction output.
    ///
    /// The `scriptCode` is the Script of the previous transaction output being serialized in the
    /// sighash when evaluating a `CHECKSIG` & co. OP code.
    /// Returns Error for Tr descriptors
    fn script_code(&self) -> Result<Script, Error>
    where
        Pk: ToPublicKey,
    {
        match *self {
            PreTaprootDescriptor::Bare(ref bare) => bare.script_code(),
            PreTaprootDescriptor::Pkh(ref pkh) => pkh.script_code(),
            PreTaprootDescriptor::Wpkh(ref wpkh) => wpkh.script_code(),
            PreTaprootDescriptor::Wsh(ref wsh) => wsh.script_code(),
            PreTaprootDescriptor::Sh(ref sh) => sh.script_code(),
        }
    }

    fn address(&self, params: &'static elements::AddressParams) -> Result<elements::Address, Error>
    where
        Pk: ToPublicKey,
    {
        match *self {
            PreTaprootDescriptor::Bare(ref bare) => bare.address(params),
            PreTaprootDescriptor::Pkh(ref pkh) => pkh.address(params),
            PreTaprootDescriptor::Wpkh(ref wpkh) => wpkh.address(params),
            PreTaprootDescriptor::Wsh(ref wsh) => wsh.address(params),
            PreTaprootDescriptor::Sh(ref sh) => sh.address(params),
        }
    }
}

impl<Pk> expression::FromTree for PreTaprootDescriptor<Pk>
where
    Pk: MiniscriptKey + str::FromStr,
    Pk::Hash: str::FromStr,
    <Pk as FromStr>::Err: ToString,
    <<Pk as MiniscriptKey>::Hash as FromStr>::Err: ToString,
{
    /// Parse an expression tree into a descriptor
    fn from_tree(top: &expression::Tree<'_>) -> Result<PreTaprootDescriptor<Pk>, Error> {
        Ok(match (top.name, top.args.len() as u32) {
            ("pkh", 1) => PreTaprootDescriptor::Pkh(Pkh::from_tree(top)?),
            ("wpkh", 1) => PreTaprootDescriptor::Wpkh(Wpkh::from_tree(top)?),
            ("sh", 1) => PreTaprootDescriptor::Sh(Sh::from_tree(top)?),
            ("wsh", 1) => PreTaprootDescriptor::Wsh(Wsh::from_tree(top)?),
            _ => PreTaprootDescriptor::Bare(Bare::from_tree(top)?),
        })
    }
}

impl<Pk> FromStr for PreTaprootDescriptor<Pk>
where
    Pk: MiniscriptKey + str::FromStr,
    Pk::Hash: str::FromStr,
    <Pk as FromStr>::Err: ToString,
    <<Pk as MiniscriptKey>::Hash as FromStr>::Err: ToString,
{
    type Err = Error;

    fn from_str(s: &str) -> Result<PreTaprootDescriptor<Pk>, Error> {
        let desc_str = verify_checksum(s)?;
        let top = expression::Tree::from_str(desc_str)?;
        expression::FromTree::from_tree(&top)
    }
}

impl<Pk: MiniscriptKey> fmt::Debug for PreTaprootDescriptor<Pk> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            PreTaprootDescriptor::Bare(ref sub) => write!(f, "{:?}", sub),
            PreTaprootDescriptor::Pkh(ref pkh) => write!(f, "{:?}", pkh),
            PreTaprootDescriptor::Wpkh(ref wpkh) => write!(f, "{:?}", wpkh),
            PreTaprootDescriptor::Sh(ref sub) => write!(f, "{:?}", sub),
            PreTaprootDescriptor::Wsh(ref sub) => write!(f, "{:?}", sub),
        }
    }
}

impl<Pk: MiniscriptKey> fmt::Display for PreTaprootDescriptor<Pk> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            PreTaprootDescriptor::Bare(ref sub) => write!(f, "{}", sub),
            PreTaprootDescriptor::Pkh(ref pkh) => write!(f, "{}", pkh),
            PreTaprootDescriptor::Wpkh(ref wpkh) => write!(f, "{}", wpkh),
            PreTaprootDescriptor::Sh(ref sub) => write!(f, "{}", sub),
            PreTaprootDescriptor::Wsh(ref sub) => write!(f, "{}", sub),
        }
    }
}

serde_string_impl_pk!(PreTaprootDescriptor, "a pre-taproot script descriptor");

// Have the trait in a separate module to avoid conflicts
pub(crate) mod traits {
    use elements::Script;

    use crate::{
        descriptor::{Pkh, Sh, Wpkh, Wsh},
        DescriptorTrait, MiniscriptKey, ToPublicKey,
    };

    use super::PreTaprootDescriptor;

    /// A general trait for Pre taproot bitcoin descriptor.
    /// Similar to [`DescriptorTrait`], but `explicit_script` and `script_code` methods cannot fail
    pub trait PreTaprootDescriptorTrait<Pk: MiniscriptKey>: DescriptorTrait<Pk> {
        /// Same as [`DescriptorTrait::explicit_script`], but a non failing version.
        /// All PreTaproot descriptors have a unique explicit script
        fn explicit_script(&self) -> Script
        where
            Pk: ToPublicKey,
        {
            // This expect can technically be avoided if we implement this for types, but
            // having this expect saves lots of LoC because of default implementation
            <Self as DescriptorTrait<Pk>>::explicit_script(self)
                .expect("Pre taproot descriptor have explicit script")
        }

        /// Same as [`DescriptorTrait::script_code`], but a non failing version.
        /// All PreTaproot descriptors have a script code
        fn script_code(&self) -> Script
        where
            Pk: ToPublicKey,
        {
            <Self as DescriptorTrait<Pk>>::script_code(self)
                .expect("Pre taproot descriptor have non-failing script code")
        }
    }

    impl<Pk: MiniscriptKey> PreTaprootDescriptorTrait<Pk> for Pkh<Pk> {}

    impl<Pk: MiniscriptKey> PreTaprootDescriptorTrait<Pk> for Sh<Pk> {}

    impl<Pk: MiniscriptKey> PreTaprootDescriptorTrait<Pk> for Wpkh<Pk> {}

    impl<Pk: MiniscriptKey> PreTaprootDescriptorTrait<Pk> for Wsh<Pk> {}

    impl<Pk: MiniscriptKey> PreTaprootDescriptorTrait<Pk> for PreTaprootDescriptor<Pk> {}
}
