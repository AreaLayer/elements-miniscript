// Miniscript
// Written in 2020 by rust-miniscript developers
//
// To the extent possible under law, the author(s) have dedicated all
// copyright and related and neighboring rights to this software to
// the public domain worldwide. This software is distributed without
// any warranty.
//
// You should have received a copy of the CC0 Public Domain Dedication
// along with this software.
// If not, see <http://creativecommons.org/publicdomain/zero/1.0/>.
//

//! # Bare Output Descriptors
//!
//! Implementation of Bare Descriptors (i.e descriptors that are)
//! wrapped inside wsh, or sh fragments.
//! Also includes pk, and pkh descriptors
//!

use std::{fmt, str::FromStr};

use elements::secp256k1_zkp;
use elements::{self, script, Script};

use expression::{self, FromTree};
use miniscript::context::ScriptContext;
use policy::{semantic, Liftable};
use util::{varint_len, witness_to_scriptsig};
use {
    BareCtx, Error, ForEach, ForEachKey, Miniscript, MiniscriptKey, Satisfier, ToPublicKey,
    TranslatePk,
};

use super::{
    checksum::{desc_checksum, verify_checksum},
    DescriptorTrait, ElementsTrait, ELMTS_STR,
};

/// Create a Bare Descriptor. That is descriptor that is
/// not wrapped in sh or wsh. This covers the Pk descriptor
#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub struct Bare<Pk: MiniscriptKey> {
    /// underlying miniscript
    ms: Miniscript<Pk, BareCtx>,
}

impl<Pk: MiniscriptKey> Bare<Pk> {
    /// Create a new raw descriptor
    pub fn new(ms: Miniscript<Pk, BareCtx>) -> Result<Self, Error> {
        // do the top-level checks
        BareCtx::top_level_checks(&ms)?;
        Ok(Self { ms: ms })
    }

    /// get the inner
    pub fn into_inner(self) -> Miniscript<Pk, BareCtx> {
        self.ms
    }

    /// get the inner
    pub fn as_inner(&self) -> &Miniscript<Pk, BareCtx> {
        &self.ms
    }
}

impl<Pk: MiniscriptKey + ToPublicKey> Bare<Pk> {
    /// Obtain the corresponding script pubkey for this descriptor
    /// Non failing verion of [`DescriptorTrait::script_pubkey`] for this descriptor
    pub fn spk(&self) -> Script {
        self.ms.encode()
    }

    /// Obtain the underlying miniscript for this descriptor
    /// Non failing verion of [`DescriptorTrait::explicit_script`] for this descriptor
    pub fn inner_script(&self) -> Script {
        self.spk()
    }

    /// Obtain the pre bip-340 signature script code for this descriptor
    /// Non failing verion of [`DescriptorTrait::script_code`] for this descriptor
    pub fn ecdsa_sighash_script_code(&self) -> Script {
        self.spk()
    }
}

impl<Pk: MiniscriptKey> fmt::Debug for Bare<Pk> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}{:?}", ELMTS_STR, self.ms)
    }
}

impl<Pk: MiniscriptKey> fmt::Display for Bare<Pk> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let desc = format!("{}{}", ELMTS_STR, self.ms);
        let checksum = desc_checksum(&desc).map_err(|_| fmt::Error)?;
        write!(f, "{}#{}", &desc, &checksum)
    }
}

impl<Pk: MiniscriptKey> Liftable<Pk> for Bare<Pk> {
    fn lift(&self) -> Result<semantic::Policy<Pk>, Error> {
        self.ms.lift()
    }
}

impl<Pk> FromTree for Bare<Pk>
where
    Pk: MiniscriptKey + FromStr,
    Pk::Hash: FromStr,
    <Pk as FromStr>::Err: ToString,
    <<Pk as MiniscriptKey>::Hash as FromStr>::Err: ToString,
{
    fn from_tree(top: &expression::Tree) -> Result<Self, Error> {
        // extra allocations to use the existing code as is.
        if top.name.starts_with("el") {
            let new_tree = expression::Tree {
                name: top.name.split_at(2).1,
                args: top.args.clone(),
            };
            let sub = Miniscript::<Pk, BareCtx>::from_tree(&new_tree)?;
            BareCtx::top_level_checks(&sub)?;
            Bare::new(sub)
        } else {
            Err(Error::Unexpected(String::from(
                "Not an elements Descriptor",
            )))
        }
    }
}

impl<Pk> FromStr for Bare<Pk>
where
    Pk: MiniscriptKey + FromStr,
    Pk::Hash: FromStr,
    <Pk as FromStr>::Err: ToString,
    <<Pk as MiniscriptKey>::Hash as FromStr>::Err: ToString,
{
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let desc_str = verify_checksum(s)?;
        let top = expression::Tree::from_str(&desc_str[2..])?;
        Self::from_tree(&top)
    }
}

impl<Pk: MiniscriptKey> ElementsTrait<Pk> for Bare<Pk> {
    fn blind_addr(
        &self,
        _blinder: Option<secp256k1_zkp::PublicKey>,
        _params: &'static elements::AddressParams,
    ) -> Result<elements::Address, Error>
    where
        Pk: ToPublicKey,
    {
        Err(Error::BareDescriptorAddr)
    }
}
impl<Pk: MiniscriptKey> DescriptorTrait<Pk> for Bare<Pk>
where
    Pk: FromStr,
    Pk::Hash: FromStr,
    <Pk as FromStr>::Err: ToString,
    <<Pk as MiniscriptKey>::Hash as FromStr>::Err: ToString,
{
    fn sanity_check(&self) -> Result<(), Error> {
        self.ms.sanity_check()?;
        Ok(())
    }

    fn address(&self, _network: &elements::AddressParams) -> Result<elements::Address, Error>
    where
        Pk: ToPublicKey,
    {
        Err(Error::BareDescriptorAddr)
    }

    fn script_pubkey(&self) -> Script
    where
        Pk: ToPublicKey,
    {
        self.spk()
    }

    fn unsigned_script_sig(&self) -> Script
    where
        Pk: ToPublicKey,
    {
        Script::new()
    }

    fn explicit_script(&self) -> Result<Script, Error>
    where
        Pk: ToPublicKey,
    {
        Ok(self.inner_script())
    }

    fn get_satisfaction<S>(&self, satisfier: S) -> Result<(Vec<Vec<u8>>, Script), Error>
    where
        Pk: ToPublicKey,
        S: Satisfier<Pk>,
    {
        let ms = self.ms.satisfy(satisfier)?;
        let script_sig = witness_to_scriptsig(&ms);
        let witness = vec![];
        Ok((witness, script_sig))
    }

    fn get_satisfaction_mall<S>(&self, satisfier: S) -> Result<(Vec<Vec<u8>>, Script), Error>
    where
        Pk: ToPublicKey,
        S: Satisfier<Pk>,
    {
        let ms = self.ms.satisfy_malleable(satisfier)?;
        let script_sig = witness_to_scriptsig(&ms);
        let witness = vec![];
        Ok((witness, script_sig))
    }

    fn max_satisfaction_weight(&self) -> Result<usize, Error> {
        let scriptsig_len = self.ms.max_satisfaction_size()?;
        Ok(4 * (varint_len(scriptsig_len) + scriptsig_len))
    }

    fn script_code(&self) -> Result<Script, Error>
    where
        Pk: ToPublicKey,
    {
        Ok(self.ecdsa_sighash_script_code())
    }
}

impl<Pk: MiniscriptKey> ForEachKey<Pk> for Bare<Pk> {
    fn for_each_key<'a, F: FnMut(ForEach<'a, Pk>) -> bool>(&'a self, pred: F) -> bool
    where
        Pk: 'a,
        Pk::Hash: 'a,
    {
        self.ms.for_each_key(pred)
    }
}

impl<P: MiniscriptKey, Q: MiniscriptKey> TranslatePk<P, Q> for Bare<P> {
    type Output = Bare<Q>;

    fn translate_pk<Fpk, Fpkh, E>(
        &self,
        mut translatefpk: Fpk,
        mut translatefpkh: Fpkh,
    ) -> Result<Self::Output, E>
    where
        Fpk: FnMut(&P) -> Result<Q, E>,
        Fpkh: FnMut(&P::Hash) -> Result<Q::Hash, E>,
        Q: MiniscriptKey,
    {
        Ok(Bare::new(
            self.ms
                .translate_pk(&mut translatefpk, &mut translatefpkh)?,
        )
        .expect("Translation cannot fail inside Bare"))
    }
}

/// A bare PkH descriptor at top level
#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub struct Pkh<Pk: MiniscriptKey> {
    /// underlying publickey
    pk: Pk,
}

impl<Pk: MiniscriptKey> Pkh<Pk> {
    /// Create a new Pkh descriptor
    pub fn new(pk: Pk) -> Self {
        // do the top-level checks
        Self { pk: pk }
    }

    /// Get a reference to the inner key
    pub fn as_inner(&self) -> &Pk {
        &self.pk
    }

    /// Get the inner key
    pub fn into_inner(self) -> Pk {
        self.pk
    }
}

impl<Pk: MiniscriptKey + ToPublicKey> Pkh<Pk> {
    /// Obtain the corresponding script pubkey for this descriptor
    /// Non failing verion of [`DescriptorTrait::script_pubkey`] for this descriptor
    pub fn spk(&self) -> Script {
        let addr = bitcoin::Address::p2pkh(&self.pk.to_public_key(), bitcoin::Network::Bitcoin);
        addr.script_pubkey()
    }

    /// Obtain the corresponding script pubkey for this descriptor
    /// Non failing verion of [`DescriptorTrait::address`] for this descriptor
    pub fn addr(&self, network: bitcoin::Network) -> bitcoin::Address {
        bitcoin::Address::p2pkh(&self.pk.to_public_key(), network)
    }

    /// Obtain the underlying miniscript for this descriptor
    /// Non failing verion of [`DescriptorTrait::explicit_script`] for this descriptor
    pub fn inner_script(&self) -> Script {
        self.spk()
    }

    /// Obtain the pre bip-340 signature script code for this descriptor
    /// Non failing verion of [`DescriptorTrait::script_code`] for this descriptor
    pub fn ecdsa_sighash_script_code(&self) -> Script {
        self.spk()
    }
}

impl<Pk: MiniscriptKey> fmt::Debug for Pkh<Pk> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}pkh({:?})", ELMTS_STR, self.pk)
    }
}

impl<Pk: MiniscriptKey> fmt::Display for Pkh<Pk> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let desc = format!("{}pkh({})", ELMTS_STR, self.pk);
        let checksum = desc_checksum(&desc).map_err(|_| fmt::Error)?;
        write!(f, "{}#{}", &desc, &checksum)
    }
}

impl<Pk: MiniscriptKey> Liftable<Pk> for Pkh<Pk> {
    fn lift(&self) -> Result<semantic::Policy<Pk>, Error> {
        Ok(semantic::Policy::KeyHash(self.pk.to_pubkeyhash()))
    }
}

impl<Pk> FromTree for Pkh<Pk>
where
    Pk: MiniscriptKey + FromStr,
    Pk::Hash: FromStr,
    <Pk as FromStr>::Err: ToString,
    <<Pk as MiniscriptKey>::Hash as FromStr>::Err: ToString,
{
    fn from_tree(top: &expression::Tree) -> Result<Self, Error> {
        if top.name == "elpkh" && top.args.len() == 1 {
            Ok(Pkh::new(expression::terminal(&top.args[0], |pk| {
                Pk::from_str(pk)
            })?))
        } else {
            Err(Error::Unexpected(format!(
                "{}({} args) while parsing pkh descriptor",
                top.name,
                top.args.len(),
            )))
        }
    }
}

impl<Pk> FromStr for Pkh<Pk>
where
    Pk: MiniscriptKey + FromStr,
    Pk::Hash: FromStr,
    <Pk as FromStr>::Err: ToString,
    <<Pk as MiniscriptKey>::Hash as FromStr>::Err: ToString,
{
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let desc_str = verify_checksum(s)?;
        let top = expression::Tree::from_str(desc_str)?;
        Self::from_tree(&top)
    }
}
impl<Pk: MiniscriptKey> ElementsTrait<Pk> for Pkh<Pk> {
    fn blind_addr(
        &self,
        blinder: Option<secp256k1_zkp::PublicKey>,
        params: &'static elements::AddressParams,
    ) -> Result<elements::Address, Error>
    where
        Pk: ToPublicKey,
    {
        Ok(elements::Address::p2pkh(
            &self.pk.to_public_key(),
            blinder,
            params,
        ))
    }
}

impl<Pk: MiniscriptKey> DescriptorTrait<Pk> for Pkh<Pk> {
    fn sanity_check(&self) -> Result<(), Error> {
        Ok(())
    }

    fn address(&self, params: &'static elements::AddressParams) -> Result<elements::Address, Error>
    where
        Pk: ToPublicKey,
    {
        // TODO: Rework address API
        Ok(elements::Address::p2pkh(
            &self.pk.to_public_key(),
            None,
            params,
        ))
    }

    fn script_pubkey(&self) -> Script
    where
        Pk: ToPublicKey,
    {
        let addr = elements::Address::p2pkh(
            &self.pk.to_public_key(),
            None,
            &elements::AddressParams::ELEMENTS,
        );
        addr.script_pubkey()
    }

    fn unsigned_script_sig(&self) -> Script
    where
        Pk: ToPublicKey,
    {
        Script::new()
    }

    fn explicit_script(&self) -> Result<Script, Error>
    where
        Pk: ToPublicKey,
    {
        Ok(self.inner_script())
    }

    fn get_satisfaction<S>(&self, satisfier: S) -> Result<(Vec<Vec<u8>>, Script), Error>
    where
        Pk: ToPublicKey,
        S: Satisfier<Pk>,
    {
        if let Some(sig) = satisfier.lookup_ecdsa_sig(&self.pk) {
            let sig_vec = sig.to_vec();
            let script_sig = script::Builder::new()
                .push_slice(&sig_vec[..])
                .push_key(&self.pk.to_public_key())
                .into_script();
            let witness = vec![];
            Ok((witness, script_sig))
        } else {
            Err(Error::MissingSig(self.pk.to_public_key()))
        }
    }

    fn get_satisfaction_mall<S>(&self, satisfier: S) -> Result<(Vec<Vec<u8>>, Script), Error>
    where
        Pk: ToPublicKey,
        S: Satisfier<Pk>,
    {
        self.get_satisfaction(satisfier)
    }

    fn max_satisfaction_weight(&self) -> Result<usize, Error> {
        Ok(4 * (1 + 73 + BareCtx::pk_len(&self.pk)))
    }

    fn script_code(&self) -> Result<Script, Error>
    where
        Pk: ToPublicKey,
    {
        Ok(self.ecdsa_sighash_script_code())
    }
}

impl<Pk: MiniscriptKey> ForEachKey<Pk> for Pkh<Pk> {
    fn for_each_key<'a, F: FnMut(ForEach<'a, Pk>) -> bool>(&'a self, mut pred: F) -> bool
    where
        Pk: 'a,
        Pk::Hash: 'a,
    {
        pred(ForEach::Key(&self.pk))
    }
}

impl<P: MiniscriptKey, Q: MiniscriptKey> TranslatePk<P, Q> for Pkh<P> {
    type Output = Pkh<Q>;

    fn translate_pk<Fpk, Fpkh, E>(
        &self,
        mut translatefpk: Fpk,
        _translatefpkh: Fpkh,
    ) -> Result<Self::Output, E>
    where
        Fpk: FnMut(&P) -> Result<Q, E>,
        Fpkh: FnMut(&P::Hash) -> Result<Q::Hash, E>,
        Q: MiniscriptKey,
    {
        Ok(Pkh::new(translatefpk(&self.pk)?))
    }
}
