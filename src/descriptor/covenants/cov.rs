// Miniscript
// Written in 2021 by
//     Andrew Poelstra <apoelstra@wpsoftware.net>
//     Sanket Kanjalkar <sanket1729@gmail.com>
// To the extent possible under law, the author(s) have dedicated all
// copyright and related and neighboring rights to this software to
// the public domain worldwide. This software is distributed without
// any warranty.
//
// You should have received a copy of the CC0 Public Domain Dedication
// along with this software.
// If not, see <http://creativecommons.org/publicdomain/zero/1.0/>.
//

//! Covenant Descriptor support
//!
//! Traits and implementations for Covenant descriptors
//! A cov() descriptor puts a context items required for
//! sighash onto the top of the stack in the required order
//!
//! ** WORKS only for Segwit sighash
//! A new transaction digest algorithm is defined, but only applicable to sigops in version 0 witness program:
//! Text from BIP 143:
//!  Double SHA256 of the serialization of:
//! 1. nVersion of the transaction (4-byte little endian)
//! 2. hashPrevouts (32-byte hash)
//! 3. hashSequence (32-byte hash)
//! 3b. ELEMENTS EXTRA hashIssuances (32-byte hash)
//! 4. outpoint (32-byte hash + 4-byte little endian)
//! 5. scriptCode of the input (serialized as scripts inside CTxOuts)
//! 6. value of the output spent by this input (8-byte little endian)
//! 7. nSequence of the input (4-byte little endian)
//! 8. hashOutputs (32-byte hash)
//! 9. nLocktime of the transaction (4-byte little endian)
//! 10. sighash type of the signature (4-byte little endian)
//!
//! The miniscript fragments lookups all the relevant fragment
//! from the stack using using OP_PICK(specifying the relative)
//! position using OP_DEPTH.
//! After all the miniscript fragments are evaluated, we concat
//! all the items using OP_CAT to obtain a Sighash on which we
//! which we verify using CHECKSIGFROMSTACK
use std::{fmt, str::FromStr};

use bitcoin;
use elements::hashes::{sha256d, Hash};
use elements::script;
use elements::secp256k1_zkp;
use elements::{
    self,
    encode::{serialize, Encodable},
    Script,
};
use miniscript::limits::{MAX_SCRIPT_SIZE, MAX_STANDARD_P2WSH_SCRIPT_SIZE};

use {
    expression::{self, FromTree},
    miniscript::{
        decode,
        lex::{lex, Token as Tk, TokenIter},
        limits::MAX_OPS_PER_SCRIPT,
        types,
    },
    util::varint_len,
    ForEach, ForEachKey, Miniscript, ScriptContext, Segwitv0, TranslatePk,
};

use super::super::{
    checksum::{desc_checksum, verify_checksum},
    ElementsTrait, ELMTS_STR,
};
use super::{CovError, CovOperations};
use {DescriptorTrait, Error, MiniscriptKey, Satisfier, ToPublicKey};

// A simple utility function to serialize an array
// of elements and compute double sha2 on it
fn hash256_arr<T: Encodable>(sl: &[T]) -> sha256d::Hash {
    let mut enc = sha256d::Hash::engine();
    for elem in sl {
        elem.consensus_encode(&mut enc).unwrap();
    }
    sha256d::Hash::from_engine(enc)
}

/// The covenant descriptor
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CovenantDescriptor<Pk: MiniscriptKey> {
    /// the pk constraining the Covenant
    /// The key over which we want CHECKSIGFROMSTACK
    pub(crate) pk: Pk,
    /// the underlying Miniscript
    /// Must be under segwit context
    pub(crate) ms: Miniscript<Pk, Segwitv0>,
}

impl<Pk: MiniscriptKey> CovenantDescriptor<Pk> {
    /// Get the pk from covenant
    pub fn pk(&self) -> &Pk {
        &self.pk
    }

    /// Get a reference to Miniscript inside covenant
    pub fn to_ms(&self) -> &Miniscript<Pk, Segwitv0> {
        &self.ms
    }

    /// Consume self and return inner miniscript
    pub fn into_ms(self) -> Miniscript<Pk, Segwitv0> {
        self.ms
    }

    /// Create a new Self from components
    pub fn new(pk: Pk, ms: Miniscript<Pk, Segwitv0>) -> Result<Self, Error> {
        // // 1) Check the 201 opcode count here
        let ms_op_count = ms.ext.ops_count_sat;
        // statically computed
        // see cov_test_limits test for the test assert
        let cov_script_ops = 24;
        let total_ops = ms_op_count.ok_or(Error::ImpossibleSatisfaction)? + cov_script_ops
            - if ms.ext.has_free_verify { 1 } else { 0 };
        if total_ops > MAX_OPS_PER_SCRIPT {
            return Err(Error::ImpossibleSatisfaction);
        }
        // 2) TODO: Sighash never exceeds 520 bytes, but we check the
        // witness script before the codesep is still under 520
        // bytes if the covenant relies on introspection of script
        let ss = 58 - if ms.ext.has_free_verify { 1 } else { 0 };
        // 3) Check that the script size does not exceed 10_000 bytes
        // global consensus rule
        if ms.script_size() + ss > MAX_SCRIPT_SIZE {
            Err(Error::ScriptSizeTooLarge)
        } else {
            Ok(Self { pk, ms })
        }
    }
    /// Encode
    pub fn encode(&self) -> Script
    where
        Pk: ToPublicKey,
    {
        let builder = self.ms.node.encode(script::Builder::new());
        builder.verify_cov(&self.pk.to_public_key()).into_script()
    }

    /// Create a satisfaction for the Covenant Descriptor
    pub fn satisfy<S: Satisfier<Pk>>(&self, s: S) -> Result<Vec<Vec<u8>>, Error>
    where
        Pk: ToPublicKey,
    {
        let mut wit = {
            use descriptor::CovError::MissingSighashItem;
            let n_version = s.lookup_nversion().ok_or(MissingSighashItem(1))?;
            let hash_prevouts = s.lookup_hashprevouts().ok_or(MissingSighashItem(1))?;
            let hash_sequence = s.lookup_hashsequence().ok_or(MissingSighashItem(3))?;
            // note the 3 again, for elements
            let hash_issuances = s.lookup_hashissuances().ok_or(MissingSighashItem(3))?;
            let outpoint = s.lookup_outpoint().ok_or(MissingSighashItem(4))?;
            let script_code = s.lookup_scriptcode().ok_or(MissingSighashItem(5))?;
            let value = s.lookup_value().ok_or(MissingSighashItem(6))?;
            let n_sequence = s.lookup_nsequence().ok_or(MissingSighashItem(7))?;
            let outputs = s.lookup_outputs().ok_or(MissingSighashItem(8))?;
            let hash_outputs = hash256_arr(outputs);
            let n_locktime = s.lookup_nlocktime().ok_or(MissingSighashItem(9))?;
            let sighash_ty = s.lookup_sighashu32().ok_or(MissingSighashItem(10))?;

            let (sig, hash_ty) = s
                .lookup_sig(&self.pk)
                .ok_or(CovError::MissingCovSignature)?;
            // Hashtype must be the same
            if sighash_ty != hash_ty.as_u32() {
                return Err(CovError::CovenantSighashTypeMismatch)?;
            }

            vec![
                Vec::from(sig.serialize_der().as_ref()), // The covenant sig
                serialize(&n_version),                   // item 1
                serialize(&hash_prevouts),               // item 2
                serialize(&hash_sequence),               // item 3
                serialize(&hash_issuances),              // ELEMENTS EXTRA: item 3b(4)
                serialize(&outpoint),                    // item 4(5)
                serialize(script_code),                  // item 5(6)
                serialize(&value),                       // item 6(7)
                serialize(&n_sequence),                  // item 7(8)
                serialize(&hash_outputs),                // item 8(9)
                serialize(&n_locktime),                  // item 9(10)
                serialize(&sighash_ty),                  // item 10(11)
            ]
        };

        let ms_wit = self.ms.satisfy(s)?;
        wit.extend(ms_wit);
        Ok(wit)
    }

    /// Script code for signing with covenant publickey.
    /// Use this script_code for sighash method when signing
    /// with the covenant pk. Use the [DescriptorTrait] script_code
    /// method for getting sighash for regular miniscripts.
    pub fn cov_script_code(&self) -> Script
    where
        Pk: ToPublicKey,
    {
        script::Builder::new().post_codesep_script().into_script()
    }
}

impl CovenantDescriptor<bitcoin::PublicKey> {
    /// Check if the given script is a covenant descriptor
    /// Consumes the iterator so that only remaining miniscript
    /// needs to be parsed from the iterator
    #[allow(unreachable_patterns)]
    fn check_cov_script(tokens: &mut TokenIter) -> Result<bitcoin::PublicKey, Error> {
        match_token!(tokens,
            Tk::CheckSigFromStack, Tk::FromAltStack, Tk::Sha256, Tk::Cat,
            Tk::Cat, Tk::Cat, Tk::Cat, Tk::Cat, Tk::Cat, Tk::Cat, Tk::Cat,
            Tk::Cat, Tk::Cat, Tk::Verify, Tk::CheckSig, Tk::CodeSep, Tk::ToAltStack,
            Tk::Dup, Tk::Pubkey(pk), Tk::Cat, Tk::Left, Tk::Num(1),
            Tk::Over, Tk::Pick, Tk::Num(11), Tk::Verify => {
                return Ok(pk);
            },
            _ => return Err(Error::CovError(CovError::BadCovDescriptor)),
        );
    }

    /// Parse a descriptor from script. While parsing
    /// other descriptors, we only parse the inner miniscript
    /// with ScriptContext. But Covenant descriptors only
    /// applicable under Wsh context to avoid implementation
    /// complexity.
    // All code for covenants can thus be separated in a module
    // This parsing is parse_insane
    pub fn parse_insane(script: &script::Script) -> Result<Self, Error> {
        let (pk, ms) = Self::parse_cov_components(script)?;
        Self::new(pk, ms)
    }

    // Utility function to parse the components of cov
    // descriptor. This allows us to parse Miniscript with
    // it's context so that it can be used with NoChecks
    // context while using the interpreter
    pub(crate) fn parse_cov_components<Ctx: ScriptContext>(
        script: &script::Script,
    ) -> Result<(bitcoin::PublicKey, Miniscript<bitcoin::PublicKey, Ctx>), Error> {
        let tokens = lex(script)?;
        let mut iter = TokenIter::new(tokens);

        let pk = CovenantDescriptor::<bitcoin::PublicKey>::check_cov_script(&mut iter)?;
        let ms = decode::parse(&mut iter)?;
        Segwitv0::check_global_validity(&ms)?;
        if ms.ty.corr.base != types::Base::B {
            return Err(Error::NonTopLevel(format!("{:?}", ms)));
        };
        if let Some(leading) = iter.next() {
            Err(Error::Trailing(leading.to_string()))
        } else {
            Ok((pk, ms))
        }
    }

    /// Parse a descriptor with additional local sanity checks.
    /// See [Miniscript::sanity_check] for all the checks. Use
    /// [parse_insane] to allow parsing insane scripts
    pub fn parse(script: &script::Script) -> Result<Self, Error> {
        let cov = Self::parse_insane(script)?;
        cov.ms.sanity_check()?;
        Ok(cov)
    }
}

impl<Pk: MiniscriptKey> FromTree for CovenantDescriptor<Pk>
where
    Pk: FromStr,
    Pk::Hash: FromStr,
    <Pk as FromStr>::Err: ToString,
    <<Pk as MiniscriptKey>::Hash as FromStr>::Err: ToString,
{
    fn from_tree(top: &expression::Tree) -> Result<Self, Error> {
        if top.name == "elcovwsh" && top.args.len() == 2 {
            let pk = expression::terminal(&top.args[0], |pk| Pk::from_str(pk))?;
            let top = &top.args[1];
            let sub = Miniscript::from_tree(&top)?;
            Segwitv0::top_level_checks(&sub)?;
            Ok(CovenantDescriptor { pk: pk, ms: sub })
        } else {
            Err(Error::Unexpected(format!(
                "{}({} args) while parsing elcovwsh descriptor",
                top.name,
                top.args.len(),
            )))
        }
    }
}
impl<Pk: MiniscriptKey> fmt::Debug for CovenantDescriptor<Pk> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}covwsh({},{})", ELMTS_STR, self.pk, self.ms)
    }
}

impl<Pk: MiniscriptKey> fmt::Display for CovenantDescriptor<Pk> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let desc = format!("{}covwsh({},{})", ELMTS_STR, self.pk, self.ms);
        let checksum = desc_checksum(&desc).map_err(|_| fmt::Error)?;
        write!(f, "{}#{}", &desc, &checksum)
    }
}

impl<Pk: MiniscriptKey> FromStr for CovenantDescriptor<Pk>
where
    Pk: FromStr,
    Pk::Hash: FromStr,
    <Pk as FromStr>::Err: ToString,
    <<Pk as MiniscriptKey>::Hash as FromStr>::Err: ToString,
{
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let desc_str = verify_checksum(s)?;
        let top = expression::Tree::from_str(desc_str)?;
        CovenantDescriptor::<Pk>::from_tree(&top)
    }
}

impl<Pk: MiniscriptKey> ElementsTrait<Pk> for CovenantDescriptor<Pk>
where
    Pk: FromStr,
    Pk::Hash: FromStr,
    <Pk as FromStr>::Err: ToString,
    <<Pk as MiniscriptKey>::Hash as FromStr>::Err: ToString,
{
    fn blind_addr(
        &self,
        blinder: Option<secp256k1_zkp::PublicKey>,
        params: &'static elements::AddressParams,
    ) -> Result<elements::Address, Error>
    where
        Pk: ToPublicKey,
    {
        Ok(elements::Address::p2wsh(
            &self.explicit_script(),
            blinder,
            params,
        ))
    }
}

impl<Pk: MiniscriptKey> DescriptorTrait<Pk> for CovenantDescriptor<Pk>
where
    Pk: FromStr,
    Pk::Hash: FromStr,
    <Pk as FromStr>::Err: ToString,
    <<Pk as MiniscriptKey>::Hash as FromStr>::Err: ToString,
{
    fn sanity_check(&self) -> Result<(), Error> {
        self.ms.sanity_check()?;
        // Additional local check for p2wsh script size
        let ss = 58 - if self.ms.ext.has_free_verify { 1 } else { 0 };
        if self.ms.script_size() + ss > MAX_STANDARD_P2WSH_SCRIPT_SIZE {
            Err(Error::ScriptSizeTooLarge)
        } else {
            Ok(())
        }
    }

    fn address(&self, params: &'static elements::AddressParams) -> Result<elements::Address, Error>
    where
        Pk: ToPublicKey,
    {
        Ok(elements::Address::p2wsh(
            &self.explicit_script(),
            None,
            params,
        ))
    }

    fn script_pubkey(&self) -> Script
    where
        Pk: ToPublicKey,
    {
        self.explicit_script().to_v0_p2wsh()
    }

    fn unsigned_script_sig(&self) -> Script
    where
        Pk: ToPublicKey,
    {
        Script::new()
    }

    fn explicit_script(&self) -> Script
    where
        Pk: ToPublicKey,
    {
        self.encode()
    }

    fn get_satisfaction<S>(&self, satisfier: S) -> Result<(Vec<Vec<u8>>, Script), Error>
    where
        Pk: ToPublicKey,
        S: Satisfier<Pk>,
    {
        let mut witness = self.satisfy(satisfier)?;
        witness.push(self.explicit_script().into_bytes());
        let script_sig = Script::new();
        Ok((witness, script_sig))
    }

    fn max_satisfaction_weight(&self) -> Result<usize, Error> {
        let script_size =
            self.ms.script_size() + 58 - if self.ms.ext.has_free_verify { 1 } else { 0 };
        let max_sat_elems = self.ms.max_satisfaction_witness_elements()? + 12;
        let max_sat_size = self.ms.max_satisfaction_size()? + 275;

        Ok(4 +  // scriptSig length byte
            varint_len(script_size) +
            script_size +
            varint_len(max_sat_elems) +
            max_sat_size)
    }

    /// This returns the entire explicit script as the script code.
    /// You will need this script code when singing with pks that
    /// inside Miniscript. Use the [cov_script_code] method to
    /// get the script code for signing with covenant pk
    fn script_code(&self) -> Script
    where
        Pk: ToPublicKey,
    {
        self.explicit_script()
    }
}

impl<Pk: MiniscriptKey> ForEachKey<Pk> for CovenantDescriptor<Pk> {
    fn for_each_key<'a, F: FnMut(ForEach<'a, Pk>) -> bool>(&'a self, mut pred: F) -> bool
    where
        Pk: 'a,
        Pk::Hash: 'a,
    {
        pred(ForEach::Key(&self.pk)) && self.ms.for_any_key(pred)
    }
}

impl<P: MiniscriptKey, Q: MiniscriptKey> TranslatePk<P, Q> for CovenantDescriptor<P> {
    type Output = CovenantDescriptor<Q>;

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
        Ok(CovenantDescriptor {
            pk: translatefpk(&self.pk)?,
            ms: self
                .ms
                .translate_pk(&mut translatefpk, &mut translatefpkh)?,
        })
    }
}
