// Miniscript
// Written in 2019 by
//     Sanket Kanjular and Andrew Poelstra
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

use bitcoin;
use bitcoin::util::taproot::TAPROOT_ANNEX_PREFIX;
use elements::hashes::{hash160, sha256, Hash};
use elements::schnorr::TapTweak;
use elements::taproot::ControlBlock;
use elements::{self, script};

use super::{stack, BitcoinKey, Error, Stack, TypedHash160};
use crate::descriptor::{CovOperations, LegacyCSFSCov};
use crate::extensions::ParseableExt;
use crate::miniscript::context::{NoChecks, ScriptContext};
use crate::util::is_v1_p2tr;
use crate::{
    BareCtx, Extension, Legacy, Miniscript, MiniscriptKey, PkTranslator, Segwitv0, Tap, TranslatePk,
};

/// Attempts to parse a slice as a Bitcoin public key, checking compressedness
/// if asked to, but otherwise dropping it
fn pk_from_slice(slice: &[u8], require_compressed: bool) -> Result<bitcoin::PublicKey, Error> {
    if let Ok(pk) = bitcoin::PublicKey::from_slice(slice) {
        if require_compressed && !pk.compressed {
            Err(Error::UncompressedPubkey)
        } else {
            Ok(pk)
        }
    } else {
        Err(Error::PubkeyParseError)
    }
}

fn pk_from_stack_elem(
    elem: &stack::Element<'_>,
    require_compressed: bool,
) -> Result<bitcoin::PublicKey, Error> {
    let slice = if let stack::Element::Push(slice) = *elem {
        slice
    } else {
        return Err(Error::PubkeyParseError);
    };
    pk_from_slice(slice, require_compressed)
}

// Parse the script with appropriate context to check for context errors like
// correct usage of x-only keys or multi_a
fn script_from_stack_elem<Ctx: ScriptContext, Ext: ParseableExt<Ctx::Key>>(
    elem: &stack::Element,
) -> Result<Miniscript<Ctx::Key, Ctx, Ext>, Error> {
    match *elem {
        stack::Element::Push(sl) => {
            Miniscript::parse_insane(&elements::Script::from(sl.to_owned())).map_err(Error::from)
        }
        stack::Element::Satisfied => {
            Miniscript::from_ast(crate::Terminal::True).map_err(Error::from)
        }
        stack::Element::Dissatisfied => {
            Miniscript::from_ast(crate::Terminal::False).map_err(Error::from)
        }
    }
}

// Try to parse covenant components from witness script
// stack element
fn cov_components_from_stackelem<Ext>(
    elem: &stack::Element<'_>,
) -> Option<(
    super::BitcoinKey,
    Miniscript<super::BitcoinKey, NoChecks, Ext>,
)>
where
    Ext: TranslatePk<BitcoinKey, bitcoin::PublicKey> + ParseableExt<BitcoinKey>,
    <Ext as TranslatePk<BitcoinKey, bitcoin::PublicKey>>::Output: ParseableExt<bitcoin::PublicKey>,
    <Ext as TranslatePk<BitcoinKey, bitcoin::PublicKey>>::Output:
        TranslatePk<bitcoin::PublicKey, BitcoinKey, Output = Ext>,
{
    let (pk, ms) = match *elem {
        stack::Element::Push(sl) => {
            LegacyCSFSCov::<
                bitcoin::PublicKey,
                <Ext as TranslatePk<BitcoinKey, bitcoin::PublicKey>>::Output,
            >::parse_cov_components(&elements::Script::from(sl.to_owned()))
            .ok()?
        }
        _ => return None,
    };
    Some((super::BitcoinKey::Fullkey(pk), ms.to_no_checks_ms()))
}

/// Helper type to indicate the origin of the bare pubkey that the interpereter uses
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum PubkeyType {
    Pk,
    Pkh,
    Wpkh,
    ShWpkh,
    Tr, // Key Spend
}

/// Helper type to indicate the origin of the bare miniscript that the interpereter uses
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ScriptType {
    Bare,
    Sh,
    Wsh,
    ShWsh,
    Tr, // Script Spend
}

/// Structure representing a script under evaluation as a Miniscript
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Inner<Ext: Extension<super::BitcoinKey>> {
    /// The script being evaluated is a simple public key check (pay-to-pk,
    /// pay-to-pkhash or pay-to-witness-pkhash)
    // Technically, this allows representing a (XonlyKey, Sh) output but we make sure
    // that only appropriate outputs are created
    PublicKey(super::BitcoinKey, PubkeyType),
    /// The script being evaluated is an actual script
    Script(Miniscript<super::BitcoinKey, NoChecks, Ext>, ScriptType),
    /// The Covenant Miniscript
    /// Only Witnessv0 scripts are supported for now
    CovScript(
        super::BitcoinKey,
        Miniscript<super::BitcoinKey, NoChecks, Ext>,
        // Add scriptType when we support additional things here
        // ScriptType,
    ),
    // todo: add extensions support as explicit enum
}

// The `Script` returned by this method is always generated/cloned ... when
// rust-bitcoin is updated to use a copy-on-write internal representation we
// should revisit this and return references to the actual txdata wherever
// possible
/// Parses an `Inner` and appropriate `Stack` from completed transaction data,
/// as well as the script that should be used as a scriptCode in a sighash
/// Tr outputs don't have script code and return None.
pub fn from_txdata<'txin, Ext: ParseableExt<BitcoinKey>>(
    spk: &elements::Script,
    script_sig: &'txin elements::Script,
    witness: &'txin [Vec<u8>],
) -> Result<(Inner<Ext>, Stack<'txin>, Option<elements::Script>), Error>
where
    Ext: TranslatePk<BitcoinKey, bitcoin::PublicKey>,
    <Ext as TranslatePk<BitcoinKey, bitcoin::PublicKey>>::Output: ParseableExt<bitcoin::PublicKey>,
    <Ext as TranslatePk<BitcoinKey, bitcoin::PublicKey>>::Output:
        TranslatePk<bitcoin::PublicKey, BitcoinKey, Output = Ext>,
    Ext: TranslatePk<BitcoinKey, bitcoin::XOnlyPublicKey>,
    <Ext as TranslatePk<BitcoinKey, bitcoin::XOnlyPublicKey>>::Output:
        ParseableExt<bitcoin::XOnlyPublicKey>,
    <Ext as TranslatePk<BitcoinKey, bitcoin::XOnlyPublicKey>>::Output:
        TranslatePk<bitcoin::XOnlyPublicKey, BitcoinKey, Output = Ext>,
{
    let mut ssig_stack: Stack<'_> = script_sig
        .instructions_minimal()
        .map(stack::Element::from_instruction)
        .collect::<Result<Vec<stack::Element<'_>>, Error>>()?
        .into();
    let mut wit_stack: Stack<'_> = witness
        .iter()
        .map(stack::Element::from)
        .collect::<Vec<stack::Element<'_>>>()
        .into();

    // ** pay to pubkey **
    if spk.is_p2pk() {
        if !wit_stack.is_empty() {
            Err(Error::NonEmptyWitness)
        } else {
            Ok((
                Inner::PublicKey(
                    pk_from_slice(&spk[1..spk.len() - 1], false)?.into(),
                    PubkeyType::Pk,
                ),
                ssig_stack,
                Some(spk.clone()),
            ))
        }
    // ** pay to pubkeyhash **
    } else if spk.is_p2pkh() {
        if !wit_stack.is_empty() {
            Err(Error::NonEmptyWitness)
        } else {
            match ssig_stack.pop() {
                Some(elem) => {
                    let pk = pk_from_stack_elem(&elem, false)?;
                    if *spk == elements::Script::new_p2pkh(&pk.to_pubkeyhash().into()) {
                        Ok((
                            Inner::PublicKey(pk.into(), PubkeyType::Pkh),
                            ssig_stack,
                            Some(spk.clone()),
                        ))
                    } else {
                        Err(Error::IncorrectPubkeyHash)
                    }
                }
                None => Err(Error::UnexpectedStackEnd),
            }
        }
    // ** pay to witness pubkeyhash **
    } else if spk.is_v0_p2wpkh() {
        if !ssig_stack.is_empty() {
            Err(Error::NonEmptyScriptSig)
        } else {
            match wit_stack.pop() {
                Some(elem) => {
                    let pk = pk_from_stack_elem(&elem, true)?;
                    if *spk == elements::Script::new_v0_wpkh(&pk.to_pubkeyhash().into()) {
                        Ok((
                            Inner::PublicKey(pk.into(), PubkeyType::Wpkh),
                            wit_stack,
                            Some(elements::Script::new_p2pkh(&pk.to_pubkeyhash().into())), // bip143, why..
                        ))
                    } else {
                        Err(Error::IncorrectWPubkeyHash)
                    }
                }
                None => Err(Error::UnexpectedStackEnd),
            }
        }
    // ** pay to witness scripthash **
    } else if spk.is_v0_p2wsh() {
        if !ssig_stack.is_empty() {
            Err(Error::NonEmptyScriptSig)
        } else {
            match wit_stack.pop() {
                Some(elem) => {
                    if let Some((pk, ms)) = cov_components_from_stackelem(&elem) {
                        let script_code =
                            script::Builder::new().post_codesep_script().into_script();
                        return Ok((Inner::CovScript(pk, ms), wit_stack, Some(script_code)));
                    }
                    let miniscript = script_from_stack_elem::<
                        Segwitv0,
                        <Ext as TranslatePk<BitcoinKey, bitcoin::PublicKey>>::Output,
                    >(&elem)?;
                    let script = miniscript.encode();
                    let miniscript =
                        <Miniscript<_, _, _> as ToNoChecks<_>>::to_no_checks_ms(&miniscript);
                    let scripthash = sha256::Hash::hash(&script[..]);
                    if *spk == elements::Script::new_v0_wsh(&scripthash.into()) {
                        Ok((
                            Inner::Script(miniscript, ScriptType::Wsh),
                            wit_stack,
                            Some(script),
                        ))
                    } else {
                        Err(Error::IncorrectWScriptHash)
                    }
                }
                None => Err(Error::UnexpectedStackEnd),
            }
        }
    // ** pay to taproot **//
    } else if is_v1_p2tr(&spk) {
        if !ssig_stack.is_empty() {
            Err(Error::NonEmptyScriptSig)
        } else {
            let output_key = bitcoin::XOnlyPublicKey::from_slice(&spk[2..])
                .map_err(|_| Error::XOnlyPublicKeyParseError)?;
            let has_annex = wit_stack
                .last()
                .and_then(|x| x.as_push().ok())
                .map(|x| !x.is_empty() && x[0] == TAPROOT_ANNEX_PREFIX)
                .unwrap_or(false);
            let has_annex = has_annex && (wit_stack.len() >= 2);
            if has_annex {
                // Annex is non-standard, bitcoin consensus rules ignore it.
                // Our sighash structure and signature verification
                // does not support annex, return error
                return Err(Error::TapAnnexUnsupported);
            }
            match wit_stack.len() {
                0 => Err(Error::UnexpectedStackEnd),
                1 => Ok((
                    Inner::PublicKey(output_key.into(), PubkeyType::Tr),
                    wit_stack,
                    None, // Tr key spend script code None
                )),
                _ => {
                    // Script spend
                    let ctrl_blk = wit_stack.pop().ok_or(Error::UnexpectedStackEnd)?;
                    let ctrl_blk = ctrl_blk.as_push()?;
                    let tap_script = wit_stack.pop().ok_or(Error::UnexpectedStackEnd)?;
                    let ctrl_blk =
                        ControlBlock::from_slice(ctrl_blk).map_err(Error::ControlBlockParse)?;
                    let tap_script = script_from_stack_elem::<
                        Tap,
                        <Ext as TranslatePk<BitcoinKey, bitcoin::XOnlyPublicKey>>::Output,
                    >(&tap_script)?;
                    let ms = tap_script.to_no_checks_ms();
                    // Creating new contexts is cheap
                    let secp = bitcoin::secp256k1::Secp256k1::verification_only();
                    let tap_script = tap_script.encode();
                    // Should not really need to call dangerous assumed tweaked here.
                    // Should be fixed after RC
                    // This is fixed in rust-bitcoin. Should also be fixed in rust-elements
                    if ctrl_blk.verify_taproot_commitment(
                        &secp,
                        &output_key.dangerous_assume_tweaked(),
                        &tap_script,
                    ) {
                        Ok((
                            Inner::Script(ms, ScriptType::Tr),
                            wit_stack,
                            // Tapscript is returned as a "scriptcode". This is a hack, but avoids adding yet
                            // another enum just for taproot, and this function is not a publicly exposed API,
                            // so it's easy enough to keep track of all uses.
                            //
                            // In particular, this return value will be put into the `script_code` member of
                            // the `Interpreter` script; the iterpreter logic does the right thing with it.
                            Some(tap_script),
                        ))
                    } else {
                        Err(Error::ControlBlockVerificationError)
                    }
                }
            }
        }
    // ** pay to scripthash **
    } else if spk.is_p2sh() {
        match ssig_stack.pop() {
            Some(elem) => {
                if let stack::Element::Push(slice) = elem {
                    let scripthash = hash160::Hash::hash(slice);
                    if *spk != elements::Script::new_p2sh(&scripthash.into()) {
                        return Err(Error::IncorrectScriptHash);
                    }
                    // ** p2sh-wrapped wpkh **
                    if slice.len() == 22 && slice[0] == 0 && slice[1] == 20 {
                        return match wit_stack.pop() {
                            Some(elem) => {
                                if !ssig_stack.is_empty() {
                                    Err(Error::NonEmptyScriptSig)
                                } else {
                                    let pk = pk_from_stack_elem(&elem, true)?;
                                    if slice
                                        == &elements::Script::new_v0_wpkh(
                                            &pk.to_pubkeyhash().into(),
                                        )[..]
                                    {
                                        Ok((
                                            Inner::PublicKey(pk.into(), PubkeyType::ShWpkh),
                                            wit_stack,
                                            Some(elements::Script::new_p2pkh(
                                                &pk.to_pubkeyhash().into(),
                                            )), // bip143, why..
                                        ))
                                    } else {
                                        Err(Error::IncorrectWScriptHash)
                                    }
                                }
                            }
                            None => Err(Error::UnexpectedStackEnd),
                        };
                    // ** p2sh-wrapped wsh **
                    } else if slice.len() == 34 && slice[0] == 0 && slice[1] == 32 {
                        return match wit_stack.pop() {
                            Some(elem) => {
                                if !ssig_stack.is_empty() {
                                    Err(Error::NonEmptyScriptSig)
                                } else {
                                    // parse wsh with Segwitv0 context
                                    let miniscript = script_from_stack_elem::<Segwitv0,                         <Ext as TranslatePk<BitcoinKey, bitcoin::PublicKey>>::Output,>(&elem)?;
                                    let script = miniscript.encode();
                                    let miniscript = miniscript.to_no_checks_ms();
                                    let scripthash = sha256::Hash::hash(&script[..]);
                                    if slice
                                        == &elements::Script::new_v0_wsh(&scripthash.into())[..]
                                    {
                                        Ok((
                                            Inner::Script(miniscript, ScriptType::ShWsh),
                                            wit_stack,
                                            Some(script),
                                        ))
                                    } else {
                                        Err(Error::IncorrectWScriptHash)
                                    }
                                }
                            }
                            None => Err(Error::UnexpectedStackEnd),
                        };
                    }
                }
                // normal p2sh parsed in Legacy context
                let miniscript = script_from_stack_elem::<
                    Legacy,
                    <Ext as TranslatePk<BitcoinKey, bitcoin::PublicKey>>::Output,
                >(&elem)?;
                let script = miniscript.encode();
                let miniscript = miniscript.to_no_checks_ms();
                if wit_stack.is_empty() {
                    let scripthash = hash160::Hash::hash(&script[..]);
                    if *spk == elements::Script::new_p2sh(&scripthash.into()) {
                        Ok((
                            Inner::Script(miniscript, ScriptType::Sh),
                            ssig_stack,
                            Some(script),
                        ))
                    } else {
                        Err(Error::IncorrectScriptHash)
                    }
                } else {
                    Err(Error::NonEmptyWitness)
                }
            }
            None => Err(Error::UnexpectedStackEnd),
        }
    // ** bare script **
    } else if wit_stack.is_empty() {
        // Bare script parsed in BareCtx
        let miniscript = Miniscript::<
            bitcoin::PublicKey,
            BareCtx,
            <Ext as TranslatePk<BitcoinKey, bitcoin::PublicKey>>::Output,
        >::parse_insane(spk)?;
        let miniscript = miniscript.to_no_checks_ms();
        Ok((
            Inner::Script(miniscript, ScriptType::Bare),
            ssig_stack,
            Some(spk.clone()),
        ))
    } else {
        Err(Error::NonEmptyWitness)
    }
}

// Convert a miniscript from a well-defined context to a no checks context.
// We need to parse insane scripts because these scripts are obtained from already
// created transaction possibly already confirmed in a block.
// In order to avoid code duplication for various contexts related interpreter checks,
// we convert all the scripts to from a well-defined context to NoContexts.
//
// While executing Pkh(<hash>) in NoChecks, we need to pop a public key from stack
// However, NoChecks context does not know whether to parse the key as 33 bytes or 32 bytes
// While converting into NoChecks we store explicitly in TypedHash160 enum.
pub(super) trait ToNoChecks<ExtQ: Extension<BitcoinKey>> {
    fn to_no_checks_ms(&self) -> Miniscript<BitcoinKey, NoChecks, ExtQ>;
}

impl<Ctx: ScriptContext, Ext: Extension<bitcoin::PublicKey>, ExtQ: Extension<BitcoinKey>>
    ToNoChecks<ExtQ> for Miniscript<bitcoin::PublicKey, Ctx, Ext>
where
    Ext: TranslatePk<bitcoin::PublicKey, BitcoinKey, Output = ExtQ>,
{
    fn to_no_checks_ms(&self) -> Miniscript<BitcoinKey, NoChecks, ExtQ> {
        struct TranslateFullPk;

        impl PkTranslator<bitcoin::PublicKey, BitcoinKey, ()> for TranslateFullPk {
            fn pk(&mut self, pk: &bitcoin::PublicKey) -> Result<BitcoinKey, ()> {
                Ok(BitcoinKey::Fullkey(*pk))
            }

            fn pkh(&mut self, pkh: &hash160::Hash) -> Result<TypedHash160, ()> {
                Ok(TypedHash160::FullKey(*pkh))
            }
        }

        self.real_translate_pk(&mut TranslateFullPk)
            .expect("Translation should succeed")
    }
}

impl<Ctx: ScriptContext, Ext: Extension<bitcoin::XOnlyPublicKey>, ExtQ: Extension<BitcoinKey>>
    ToNoChecks<ExtQ> for Miniscript<bitcoin::XOnlyPublicKey, Ctx, Ext>
where
    Ext: TranslatePk<bitcoin::XOnlyPublicKey, BitcoinKey, Output = ExtQ>,
{
    fn to_no_checks_ms(&self) -> Miniscript<BitcoinKey, NoChecks, ExtQ> {
        // specify the () error type as this cannot error
        struct TranslateXOnlyPk;

        impl PkTranslator<bitcoin::XOnlyPublicKey, BitcoinKey, ()> for TranslateXOnlyPk {
            fn pk(&mut self, pk: &bitcoin::XOnlyPublicKey) -> Result<BitcoinKey, ()> {
                Ok(BitcoinKey::XOnlyPublicKey(*pk))
            }

            fn pkh(&mut self, pkh: &hash160::Hash) -> Result<TypedHash160, ()> {
                Ok(TypedHash160::XonlyKey(*pkh))
            }
        }
        self.real_translate_pk(&mut TranslateXOnlyPk)
            .expect("Translation should succeed")
    }
}

#[cfg(test)]
mod tests {

    use std::str::FromStr;

    use elements::hashes::hex::FromHex;
    use elements::hashes::{hash160, sha256, Hash};
    use elements::{self, script, Script};

    use super::*;
    use crate::CovenantExt;

    struct KeyTestData {
        pk_spk: elements::Script,
        pk_sig: elements::Script,
        pkh_spk: elements::Script,
        pkh_sig: elements::Script,
        pkh_sig_justkey: elements::Script,
        wpkh_spk: elements::Script,
        wpkh_stack: Vec<Vec<u8>>,
        wpkh_stack_justkey: Vec<Vec<u8>>,
        sh_wpkh_spk: elements::Script,
        sh_wpkh_sig: elements::Script,
        sh_wpkh_stack: Vec<Vec<u8>>,
        sh_wpkh_stack_justkey: Vec<Vec<u8>>,
    }

    impl KeyTestData {
        fn from_key(key: bitcoin::PublicKey) -> KeyTestData {
            // what a funny looking signature..
            let dummy_sig = Vec::from_hex(
                "\
                302e02153b78ce563f89a0ed9414f5aa28ad0d96d6795f9c63\
                    02153b78ce563f89a0ed9414f5aa28ad0d96d6795f9c65\
            ",
            )
            .unwrap();

            let pkhash = key.to_pubkeyhash().into();
            let wpkhash = key.to_pubkeyhash().into();
            let wpkh_spk = elements::Script::new_v0_wpkh(&wpkhash);
            let wpkh_scripthash = hash160::Hash::hash(&wpkh_spk[..]).into();

            KeyTestData {
                pk_spk: elements::Script::new_p2pk(&key),
                pkh_spk: elements::Script::new_p2pkh(&pkhash),
                pk_sig: script::Builder::new().push_slice(&dummy_sig).into_script(),
                pkh_sig: script::Builder::new()
                    .push_slice(&dummy_sig)
                    .push_key(&key)
                    .into_script(),
                pkh_sig_justkey: script::Builder::new().push_key(&key).into_script(),
                wpkh_spk: wpkh_spk.clone(),
                wpkh_stack: vec![dummy_sig.clone(), key.to_bytes()],
                wpkh_stack_justkey: vec![key.to_bytes()],
                sh_wpkh_spk: elements::Script::new_p2sh(&wpkh_scripthash),
                sh_wpkh_sig: script::Builder::new()
                    .push_slice(&wpkh_spk[..])
                    .into_script(),
                sh_wpkh_stack: vec![dummy_sig, key.to_bytes()],
                sh_wpkh_stack_justkey: vec![key.to_bytes()],
            }
        }
    }

    struct FixedTestData {
        pk_comp: bitcoin::PublicKey,
        pk_uncomp: bitcoin::PublicKey,
    }

    fn fixed_test_data() -> FixedTestData {
        FixedTestData {
            pk_comp: bitcoin::PublicKey::from_str(
                "\
                025edd5cc23c51e87a497ca815d5dce0f8ab52554f849ed8995de64c5f34ce7143\
            ",
            )
            .unwrap(),
            pk_uncomp: bitcoin::PublicKey::from_str(
                "\
                045edd5cc23c51e87a497ca815d5dce0f8ab52554f849ed8995de64c5f34ce7143\
                  efae9c8dbc14130661e8cec030c89ad0c13c66c0d17a2905cdc706ab7399a868\
            ",
            )
            .unwrap(),
        }
    }

    #[test]
    fn pubkey_pk() {
        let fixed = fixed_test_data();
        let comp = KeyTestData::from_key(fixed.pk_comp);
        let uncomp = KeyTestData::from_key(fixed.pk_uncomp);
        let blank_script = elements::Script::new();

        // Compressed pk, empty scriptsig
        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&comp.pk_spk, &blank_script, &[]).expect("parse txdata");
        assert_eq!(
            inner,
            Inner::PublicKey(fixed.pk_comp.into(), PubkeyType::Pk)
        );
        assert_eq!(stack, Stack::from(vec![]));
        assert_eq!(script_code, Some(comp.pk_spk.clone()));

        // Uncompressed pk, empty scriptsig
        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&uncomp.pk_spk, &blank_script, &[]).expect("parse txdata");
        assert_eq!(
            inner,
            Inner::PublicKey(fixed.pk_uncomp.into(), PubkeyType::Pk)
        );
        assert_eq!(stack, Stack::from(vec![]));
        assert_eq!(script_code, Some(uncomp.pk_spk.clone()));

        // Compressed pk, correct scriptsig
        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&comp.pk_spk, &comp.pk_sig, &[]).expect("parse txdata");
        assert_eq!(
            inner,
            Inner::PublicKey(fixed.pk_comp.into(), PubkeyType::Pk)
        );
        assert_eq!(stack, Stack::from(vec![comp.pk_sig[1..].into()]));
        assert_eq!(script_code, Some(comp.pk_spk.clone()));

        // Uncompressed pk, correct scriptsig
        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&uncomp.pk_spk, &uncomp.pk_sig, &[]).expect("parse txdata");
        assert_eq!(
            inner,
            Inner::PublicKey(fixed.pk_uncomp.into(), PubkeyType::Pk)
        );
        assert_eq!(stack, Stack::from(vec![uncomp.pk_sig[1..].into()]));
        assert_eq!(script_code, Some(uncomp.pk_spk));

        // Scriptpubkey has invalid key
        let mut spk = comp.pk_spk.to_bytes();
        spk[1] = 5;
        let spk = elements::Script::from(spk);
        let err = from_txdata::<CovenantExt>(&spk, &elements::Script::new(), &[]).unwrap_err();
        assert_eq!(err.to_string(), "could not parse pubkey");

        // Scriptpubkey has invalid script
        let mut spk = comp.pk_spk.to_bytes();
        spk[0] = 100;
        let spk = elements::Script::from(spk);
        let err = from_txdata::<CovenantExt>(&spk, &elements::Script::new(), &[]).unwrap_err();
        assert_eq!(&err.to_string()[0..12], "parse error:");

        // Witness is nonempty

        let err = from_txdata::<CovenantExt>(&comp.pk_spk, &comp.pk_sig, &[vec![]]).unwrap_err();

        assert_eq!(err.to_string(), "legacy spend had nonempty witness");
    }

    #[test]
    fn pubkey_pkh() {
        let fixed = fixed_test_data();
        let comp = KeyTestData::from_key(fixed.pk_comp);
        let uncomp = KeyTestData::from_key(fixed.pk_uncomp);

        // pkh, empty scriptsig; this time it errors out
        let err =
            from_txdata::<CovenantExt>(&comp.pkh_spk, &elements::Script::new(), &[]).unwrap_err();
        assert_eq!(err.to_string(), "unexpected end of stack");

        // pkh, wrong pubkey
        let err =
            from_txdata::<CovenantExt>(&comp.pkh_spk, &uncomp.pkh_sig_justkey, &[]).unwrap_err();
        assert_eq!(err.to_string(), "public key did not match scriptpubkey");

        // pkh, right pubkey, no signature
        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&comp.pkh_spk, &comp.pkh_sig_justkey, &[])
                .expect("parse txdata");
        assert_eq!(
            inner,
            Inner::PublicKey(fixed.pk_comp.into(), PubkeyType::Pkh)
        );
        assert_eq!(stack, Stack::from(vec![]));
        assert_eq!(script_code, Some(comp.pkh_spk.clone()));

        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&uncomp.pkh_spk, &uncomp.pkh_sig_justkey, &[])
                .expect("parse txdata");
        assert_eq!(
            inner,
            Inner::PublicKey(fixed.pk_uncomp.into(), PubkeyType::Pkh)
        );
        assert_eq!(stack, Stack::from(vec![]));
        assert_eq!(script_code, Some(uncomp.pkh_spk.clone()));

        // pkh, right pubkey, signature
        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&comp.pkh_spk, &comp.pkh_sig_justkey, &[])
                .expect("parse txdata");
        assert_eq!(
            inner,
            Inner::PublicKey(fixed.pk_comp.into(), PubkeyType::Pkh)
        );
        assert_eq!(stack, Stack::from(vec![]));
        assert_eq!(script_code, Some(comp.pkh_spk.clone()));

        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&uncomp.pkh_spk, &uncomp.pkh_sig_justkey, &[])
                .expect("parse txdata");
        assert_eq!(
            inner,
            Inner::PublicKey(fixed.pk_uncomp.into(), PubkeyType::Pkh)
        );
        assert_eq!(stack, Stack::from(vec![]));
        assert_eq!(script_code, Some(uncomp.pkh_spk.clone()));

        // Witness is nonempty
        let err = from_txdata::<CovenantExt>(&comp.pkh_spk, &comp.pkh_sig, &[vec![]]).unwrap_err();
        assert_eq!(err.to_string(), "legacy spend had nonempty witness");
    }

    #[test]
    fn pubkey_wpkh() {
        let fixed = fixed_test_data();
        let comp = KeyTestData::from_key(fixed.pk_comp);
        let uncomp = KeyTestData::from_key(fixed.pk_uncomp);
        let blank_script = elements::Script::new();

        // wpkh, empty witness; this time it errors out
        let err = from_txdata::<CovenantExt>(&comp.wpkh_spk, &blank_script, &[]).unwrap_err();
        assert_eq!(err.to_string(), "unexpected end of stack");

        // wpkh, uncompressed pubkey
        let err =
            from_txdata::<CovenantExt>(&comp.wpkh_spk, &blank_script, &uncomp.wpkh_stack_justkey)
                .unwrap_err();
        assert_eq!(
            err.to_string(),
            "uncompressed pubkey in non-legacy descriptor"
        );

        // wpkh, wrong pubkey
        let err =
            from_txdata::<CovenantExt>(&uncomp.wpkh_spk, &blank_script, &comp.wpkh_stack_justkey)
                .unwrap_err();
        assert_eq!(
            err.to_string(),
            "public key did not match scriptpubkey (segwit v0)"
        );

        // wpkh, right pubkey, no signature
        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&comp.wpkh_spk, &blank_script, &comp.wpkh_stack_justkey)
                .expect("parse txdata");
        assert_eq!(
            inner,
            Inner::PublicKey(fixed.pk_comp.into(), PubkeyType::Wpkh)
        );
        assert_eq!(stack, Stack::from(vec![]));
        assert_eq!(script_code, Some(comp.pkh_spk.clone()));

        // wpkh, right pubkey, signature
        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&comp.wpkh_spk, &blank_script, &comp.wpkh_stack)
                .expect("parse txdata");
        assert_eq!(
            inner,
            Inner::PublicKey(fixed.pk_comp.into(), PubkeyType::Wpkh)
        );
        assert_eq!(
            stack,
            Stack::from(vec![comp.wpkh_stack[comp.wpkh_stack.len() - 2][..].into()])
        );
        assert_eq!(script_code, Some(comp.pkh_spk.clone()));

        assert_eq!(
            inner,
            Inner::PublicKey(fixed.pk_comp.into(), PubkeyType::Wpkh)
        );
        assert_eq!(stack, Stack::from(vec![comp.wpkh_stack[0][..].into()]));
        assert_eq!(script_code, Some(comp.pkh_spk));

        // Scriptsig is nonempty
        let err =
            from_txdata::<CovenantExt>(&comp.wpkh_spk, &comp.pk_sig, &comp.wpkh_stack_justkey)
                .unwrap_err();
        assert_eq!(err.to_string(), "segwit spend had nonempty scriptsig");
    }

    #[test]
    fn pubkey_sh_wpkh() {
        let fixed = fixed_test_data();
        let comp = KeyTestData::from_key(fixed.pk_comp);
        let uncomp = KeyTestData::from_key(fixed.pk_uncomp);
        let blank_script = elements::Script::new();

        // sh_wpkh, missing witness or scriptsig

        let err = from_txdata::<CovenantExt>(&comp.sh_wpkh_spk, &blank_script, &[]).unwrap_err();
        assert_eq!(err.to_string(), "unexpected end of stack");
        let err =
            from_txdata::<CovenantExt>(&comp.sh_wpkh_spk, &comp.sh_wpkh_sig, &[]).unwrap_err();

        assert_eq!(err.to_string(), "unexpected end of stack");
        let err = from_txdata::<CovenantExt>(&comp.sh_wpkh_spk, &blank_script, &comp.sh_wpkh_stack)
            .unwrap_err();
        assert_eq!(err.to_string(), "unexpected end of stack");

        // sh_wpkh, uncompressed pubkey
        let err = from_txdata::<CovenantExt>(
            &uncomp.sh_wpkh_spk,
            &uncomp.sh_wpkh_sig,
            &uncomp.sh_wpkh_stack_justkey,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "uncompressed pubkey in non-legacy descriptor"
        );

        // sh_wpkh, wrong redeem script for scriptpubkey
        let err = from_txdata::<CovenantExt>(
            &uncomp.sh_wpkh_spk,
            &comp.sh_wpkh_sig,
            &comp.sh_wpkh_stack_justkey,
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "redeem script did not match scriptpubkey",);

        // sh_wpkh, wrong redeem script for witness script
        let err = from_txdata::<CovenantExt>(
            &uncomp.sh_wpkh_spk,
            &uncomp.sh_wpkh_sig,
            &comp.sh_wpkh_stack_justkey,
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "witness script did not match scriptpubkey",);

        // sh_wpkh, right pubkey, no signature
        let (inner, stack, script_code) = from_txdata::<CovenantExt>(
            &comp.sh_wpkh_spk,
            &comp.sh_wpkh_sig,
            &comp.sh_wpkh_stack_justkey,
        )
        .expect("parse txdata");
        assert_eq!(
            inner,
            Inner::PublicKey(fixed.pk_comp.into(), PubkeyType::ShWpkh)
        );
        assert_eq!(stack, Stack::from(vec![]));
        assert_eq!(script_code, Some(comp.pkh_spk.clone()));

        // sh_wpkh, right pubkey, signature
        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&comp.sh_wpkh_spk, &comp.sh_wpkh_sig, &comp.sh_wpkh_stack)
                .expect("parse txdata");
        assert_eq!(
            inner,
            Inner::PublicKey(fixed.pk_comp.into(), PubkeyType::ShWpkh)
        );
        assert_eq!(
            stack,
            Stack::from(vec![comp.wpkh_stack[comp.wpkh_stack.len() - 2][..].into()])
        );
        assert_eq!(script_code, Some(comp.pkh_spk.clone()));
    }

    fn ms_inner_script(
        ms: &str,
    ) -> (
        Miniscript<BitcoinKey, NoChecks, CovenantExt>,
        elements::Script,
    ) {
        let ms =
            Miniscript::<bitcoin::PublicKey, Segwitv0, CovenantExt>::from_str_insane(ms).unwrap();
        let spk = ms.encode();
        let miniscript = ms.to_no_checks_ms();
        (miniscript, spk)
    }
    #[test]
    fn script_bare() {
        let preimage = b"12345678----____12345678----____";
        let hash = hash160::Hash::hash(&preimage[..]);

        let (miniscript, spk) = ms_inner_script(&format!("hash160({})", hash));

        let blank_script = elements::Script::new();

        // bare script has no validity requirements beyond being a sane script
        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&spk, &blank_script, &[]).expect("parse txdata");
        assert_eq!(inner, Inner::Script(miniscript, ScriptType::Bare));
        assert_eq!(stack, Stack::from(vec![]));
        assert_eq!(script_code, Some(spk.clone()));

        let err = from_txdata::<CovenantExt>(&blank_script, &blank_script, &[]).unwrap_err();
        assert_eq!(&err.to_string()[0..12], "parse error:");

        // nonempty witness
        let err = from_txdata::<CovenantExt>(&spk, &blank_script, &[vec![]]).unwrap_err();
        assert_eq!(&err.to_string(), "legacy spend had nonempty witness");
    }

    #[test]
    fn script_sh() {
        let preimage = b"12345678----____12345678----____";
        let hash = hash160::Hash::hash(&preimage[..]);
        let (miniscript, redeem_script) = ms_inner_script(&format!("hash160({})", hash));
        let rs_hash = hash160::Hash::hash(&redeem_script[..]).into();

        let spk = Script::new_p2sh(&rs_hash);
        let script_sig = script::Builder::new()
            .push_slice(&redeem_script[..])
            .into_script();
        let blank_script = elements::Script::new();

        // sh without scriptsig

        let err = from_txdata::<CovenantExt>(&spk, &blank_script, &[]).unwrap_err();
        assert_eq!(&err.to_string(), "unexpected end of stack");

        // with incorrect scriptsig
        let err = from_txdata::<CovenantExt>(&spk, &spk, &[]).unwrap_err();
        assert_eq!(&err.to_string(), "expected push in script");

        // with correct scriptsig
        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&spk, &script_sig, &[]).expect("parse txdata");
        assert_eq!(inner, Inner::Script(miniscript, ScriptType::Sh));
        assert_eq!(stack, Stack::from(vec![]));
        assert_eq!(script_code, Some(redeem_script.clone()));

        // nonempty witness
        let err = from_txdata::<CovenantExt>(&spk, &script_sig, &[vec![]]).unwrap_err();
        assert_eq!(&err.to_string(), "legacy spend had nonempty witness");
    }

    #[test]
    fn script_wsh() {
        let preimage = b"12345678----____12345678----____";
        let hash = hash160::Hash::hash(&preimage[..]);

        let (miniscript, witness_script) = ms_inner_script(&format!("hash160({})", hash));

        let wit_hash = sha256::Hash::hash(&witness_script[..]).into();
        let wit_stack = vec![witness_script.to_bytes()];

        let spk = Script::new_v0_wsh(&wit_hash);
        let blank_script = elements::Script::new();

        // wsh without witness
        let err = from_txdata::<CovenantExt>(&spk, &blank_script, &[]).unwrap_err();
        assert_eq!(&err.to_string(), "unexpected end of stack");

        // with incorrect witness
        let err = from_txdata::<CovenantExt>(&spk, &blank_script, &[spk.to_bytes()]).unwrap_err();
        assert_eq!(&err.to_string()[0..12], "parse error:");

        // with correct witness
        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&spk, &blank_script, &wit_stack).expect("parse txdata");
        assert_eq!(inner, Inner::Script(miniscript, ScriptType::Wsh));
        assert_eq!(stack, Stack::from(vec![]));
        assert_eq!(script_code, Some(witness_script.clone()));

        // nonempty script_sig
        let script_sig = script::Builder::new()
            .push_slice(&witness_script[..])
            .into_script();
        let err = from_txdata::<CovenantExt>(&spk, &script_sig, &wit_stack).unwrap_err();
        assert_eq!(&err.to_string(), "segwit spend had nonempty scriptsig");
    }

    #[test]
    fn script_sh_wsh() {
        let preimage = b"12345678----____12345678----____";
        let hash = hash160::Hash::hash(&preimage[..]);

        let (miniscript, witness_script) = ms_inner_script(&format!("hash160({})", hash));

        let wit_hash = sha256::Hash::hash(&witness_script[..]).into();
        let wit_stack = vec![witness_script.to_bytes()];

        let redeem_script = Script::new_v0_wsh(&wit_hash);
        let script_sig = script::Builder::new()
            .push_slice(&redeem_script[..])
            .into_script();
        let blank_script = elements::Script::new();

        let rs_hash = hash160::Hash::hash(&redeem_script[..]).into();
        let spk = Script::new_p2sh(&rs_hash);

        // shwsh without witness or scriptsig

        let err = from_txdata::<CovenantExt>(&spk, &blank_script, &[]).unwrap_err();
        assert_eq!(&err.to_string(), "unexpected end of stack");
        let err = from_txdata::<CovenantExt>(&spk, &script_sig, &[]).unwrap_err();
        assert_eq!(&err.to_string(), "unexpected end of stack");
        let err = from_txdata::<CovenantExt>(&spk, &blank_script, &wit_stack).unwrap_err();
        assert_eq!(&err.to_string(), "unexpected end of stack");

        // with incorrect witness

        let err = from_txdata::<CovenantExt>(&spk, &script_sig, &[spk.to_bytes()]).unwrap_err();
        assert_eq!(&err.to_string()[0..12], "parse error:");

        // with incorrect scriptsig
        let err = from_txdata::<CovenantExt>(&spk, &redeem_script, &wit_stack).unwrap_err();
        assert_eq!(&err.to_string(), "redeem script did not match scriptpubkey");

        // with correct witness
        let (inner, stack, script_code) =
            from_txdata::<CovenantExt>(&spk, &script_sig, &wit_stack).expect("parse txdata");
        assert_eq!(inner, Inner::Script(miniscript, ScriptType::ShWsh));
        assert_eq!(stack, Stack::from(vec![]));
        assert_eq!(script_code, Some(witness_script));
    }
}
