//! Signature policies (CAdES `signature-policy-identifier`, RFC 5126 §5.8.1): the ICP-Brasil
//! PAdES policy that Brazil's validator (validar.iti.gov.br) checks.
//!
//! The values come from ITI's published policy files: `PA_PAdES_AD_RB_v1_3.der` (SHA-256 below,
//! as listed in `LPA_PAdES.der` version 2.0 and in `PA_PAdES_AD_RB_v1_3der-sha256.txt`), valid
//! from 2025-07-23 to 2037-10-22. The policy requires the signed attributes content-type,
//! message-digest, signing-certificate-v2 and signature-policy-identifier, and a signature
//! dictionary with `/Filter /PBAD_PAdES` and `/SubFilter /PBAD.PAdES`.

use crate::der;
use crate::x509::Certificate;

/// The CAdES `id-aa-ets-sigPolicyId` attribute.
pub const SIG_POLICY_ID_ATTR: &str = "1.2.840.113549.1.9.16.2.15";
/// `id-spq-ets-uri`: the policy qualifier giving where the policy can be read.
const SPQ_URI: &str = "1.2.840.113549.1.9.16.5.1";
const SHA256: &str = "2.16.840.1.101.3.4.2.1";

/// A signature policy UniPDF can sign under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignaturePolicy {
    /// ICP-Brasil PAdES "Assinatura Digital com Referência Básica" (AD-RB), version 1.3.
    IcpBrasilAdRb,
}

impl SignaturePolicy {
    /// The policy's OID (`sigPolicyId`).
    pub fn oid(self) -> &'static str {
        match self {
            Self::IcpBrasilAdRb => "2.16.76.1.7.1.11.1.3",
        }
    }

    /// Where the policy is published (the `SPuri` qualifier).
    pub fn uri(self) -> &'static str {
        match self {
            Self::IcpBrasilAdRb => "http://politicas.icpbrasil.gov.br/PA_PAdES_AD_RB_v1_3.der",
        }
    }

    /// SHA-256 of the policy file (`sigPolicyHash`).
    pub fn sha256(self) -> [u8; 32] {
        match self {
            Self::IcpBrasilAdRb => [
                0x23, 0xDA, 0x54, 0x4A, 0xEF, 0x71, 0xF7, 0xA7, 0x5D, 0xC8, 0x5F, 0xA6, 0xE1, 0x7A, 0x83, 0x87, 0x57, 0x41, 0xE4, 0xBA, 0xEF, 0x41,
                0xEC, 0x17, 0x82, 0x58, 0xA5, 0xC8, 0x6A, 0xCE, 0x54, 0xDD,
            ],
        }
    }

    /// A short name for people: the one ITI's report uses.
    pub fn name(self) -> &'static str {
        match self {
            Self::IcpBrasilAdRb => "ICP-Brasil AD-RB (PA_PAdES_AD_RB v1.3)",
        }
    }

    /// The signature dictionary's `/Filter` and `/SubFilter` the policy requires.
    pub fn filters(self) -> (&'static str, &'static str) {
        match self {
            Self::IcpBrasilAdRb => ("PBAD_PAdES", "PBAD.PAdES"),
        }
    }

    /// The policy a `sigPolicyId` OID names, if it is one UniPDF knows.
    pub fn from_oid(oid: &str) -> Option<Self> {
        (oid == Self::IcpBrasilAdRb.oid()).then_some(Self::IcpBrasilAdRb)
    }

    /// The DER value of the `signature-policy-identifier` attribute:
    /// `SignaturePolicyId ::= SEQUENCE { sigPolicyId, sigPolicyHash, sigPolicyQualifiers }`.
    pub fn attribute_value(self) -> Vec<u8> {
        let hash = der::seq(&[&der::algorithm(SHA256, None), &der::octets(&self.sha256())]);
        let qualifier = der::seq(&[&der::oid(SPQ_URI), &der::ia5(self.uri())]);
        der::seq(&[&der::oid(self.oid()), &hash, &der::seq(&[&qualifier])])
    }
}

/// Whether `cert` was issued under ICP-Brasil: every ICP-Brasil certification authority, from
/// the root ("Autoridade Certificadora Raiz Brasileira") down, has `O=ICP-Brasil` in its name.
pub fn is_icp_brasil(cert: &Certificate) -> bool {
    cert.issuer.organization() == Some("ICP-Brasil")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_identifier_matches_itis_published_policy() {
        let p = SignaturePolicy::IcpBrasilAdRb;
        assert_eq!(SignaturePolicy::from_oid("2.16.76.1.7.1.11.1.3"), Some(p));
        assert_eq!(SignaturePolicy::from_oid("2.16.76.1.7.1.11.1.1"), None);
        let hex: String = p.sha256().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "23da544aef71f7a75dc85fa6e17a83875741e4baef41ec178258a5c86ace54dd");
        let v = p.attribute_value();
        let t = der::Tlv::parse_all(&v).unwrap();
        let parts = t.children().unwrap();
        assert_eq!(parts[0].oid().unwrap(), "2.16.76.1.7.1.11.1.3");
        let hash = parts[1].children().unwrap();
        assert_eq!(hash[1].value, p.sha256());
        let qualifiers = parts[2].children().unwrap();
        let q = qualifiers[0].children().unwrap();
        assert_eq!(q[0].oid().unwrap(), "1.2.840.113549.1.9.16.5.1");
        assert_eq!(q[1].value, p.uri().as_bytes());
    }
}
