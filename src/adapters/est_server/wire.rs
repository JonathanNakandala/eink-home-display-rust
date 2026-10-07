//! The shapes EST puts on the wire (RFC 7030 as corrected by RFC 8951): base64 text holding DER.
//!
//! A response with certificates is a "certs-only" CMS message, a signed-data structure with nothing
//! signed and nothing in it but certificates, the form every EST client expects. The list of what a
//! request should contain is the `CsrAttrs` sequence of RFC 7030 section 4.5.2 in the form RFC 8951
//! gives. RFC 9908's request template is not used: it has no way to say that a request must carry the
//! channel binding, and this server asks for that.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use yasna::models::ObjectIdentifier;
use yasna::{Tag, construct_der};

/// What a certs-only response is labelled (RFC 7030 section 4.2.3, RFC 5273).
pub const CERTS_ONLY: &str = "application/pkcs7-mime; smime-type=certs-only";
pub const PKCS10: &str = "application/pkcs10";
pub const CSR_ATTRIBUTES: &str = "application/csrattrs";

const SIGNED_DATA: &[u64] = &[1, 2, 840, 113549, 1, 7, 2];
const DATA: &[u64] = &[1, 2, 840, 113549, 1, 7, 1];
const CHALLENGE_PASSWORD: &[u64] = &[1, 2, 840, 113549, 1, 9, 7];
const EC_PUBLIC_KEY: &[u64] = &[1, 2, 840, 10045, 2, 1];
const PRIME256V1: &[u64] = &[1, 2, 840, 10045, 3, 1, 7];
const ECDSA_SHA256: &[u64] = &[1, 2, 840, 10045, 4, 3, 2];

fn oid(components: &[u64]) -> ObjectIdentifier {
    ObjectIdentifier::from_slice(components)
}

/// DER of a CMS `SignedData` holding `certificates` and nothing else.
pub fn certs_only(certificates: &[&[u8]]) -> Vec<u8> {
    construct_der(|writer| {
        writer.write_sequence(|writer| {
            writer.next().write_oid(&oid(SIGNED_DATA));
            writer.next().write_tagged(Tag::context(0), |writer| {
                writer.write_sequence(|writer| {
                    writer.next().write_i64(1);
                    writer.next().write_set(|_| {});
                    writer
                        .next()
                        .write_sequence(|writer| writer.next().write_oid(&oid(DATA)));
                    writer
                        .next()
                        .write_tagged_implicit(Tag::context(0), |writer| {
                            writer.write_set(|writer| {
                                for certificate in certificates {
                                    writer.next().write_der(certificate);
                                }
                            });
                        });
                    writer.next().write_set(|_| {});
                });
            });
        });
    })
}

/// What the server asks a request to be: an ECDSA key on P-256, signed with SHA-256, and, if it
/// requires the channel binding, the challenge-password OID that says so (RFC 7030 section 4.5.2).
pub fn csr_attributes(require_channel_binding: bool) -> Vec<u8> {
    construct_der(|writer| {
        writer.write_sequence(|writer| {
            if require_channel_binding {
                writer.next().write_oid(&oid(CHALLENGE_PASSWORD));
            }
            writer.next().write_sequence(|writer| {
                writer.next().write_oid(&oid(EC_PUBLIC_KEY));
                writer
                    .next()
                    .write_set(|writer| writer.next().write_oid(&oid(PRIME256V1)));
            });
            writer.next().write_oid(&oid(ECDSA_SHA256));
        });
    })
}

/// The body of a response: base64 of DER, on one line (white space is allowed but not needed).
pub fn encode_body(der: &[u8]) -> String {
    STANDARD.encode(der)
}

/// The DER in a request body. Line breaks and spaces are tolerated, as RFC 8951 section 3.1 asks of
/// a receiver, since MIME's base64 is wrapped.
pub fn decode_body(body: &[u8]) -> Option<Vec<u8>> {
    let compact: Vec<u8> = body
        .iter()
        .copied()
        .filter(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        .collect();
    STANDARD.decode(compact).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn the_attribute_list_is_the_shape_rfc_8951_shows() {
        // RFC 8951's example is a SEQUENCE of: the challengePassword OID; a SEQUENCE of the ecPublicKey OID
        // and a SET holding the curve; the signature algorithm OID. The same, for P-256 and SHA-256.
        assert_eq!(
            hex(&csr_attributes(true)),
            [
                "302c",
                "0609",
                "2a864886f70d010907", // challengePassword
                "3015",
                "0607",
                "2a8648ce3d0201", // ecPublicKey ...
                "310a",
                "0608",
                "2a8648ce3d030107", // ... { prime256v1 }
                "0608",
                "2a8648ce3d040302", // ecdsaWithSHA256
            ]
            .concat()
        );
    }

    #[test]
    fn without_the_binding_requirement_the_password_oid_is_left_out() {
        assert_eq!(
            hex(&csr_attributes(false)),
            [
                "3021",
                "3015",
                "0607",
                "2a8648ce3d0201",
                "310a",
                "0608",
                "2a8648ce3d030107",
                "0608",
                "2a8648ce3d040302",
            ]
            .concat()
        );
    }

    #[test]
    fn rfc_8951s_own_example_decodes_to_what_it_says() {
        // The base64 from RFC 8951 section 4, as the text of an `application/csrattrs` body.
        let example = "MEEGCSqGSIb3DQEJBzASBgcqhkjOPQIBMQcGBSuBBAAiMBYGCSqGSIb3DQEJDjEJ\nBgcrBgEBAQEWBggqhkjOPQQDAw==";
        let der = decode_body(example.as_bytes()).unwrap();
        assert_eq!(der.len(), 0x41 + 2);
        assert_eq!(&der[..2], [0x30, 0x41]);
    }

    #[test]
    fn a_body_may_be_wrapped_and_spaced() {
        let der = vec![7u8; 100];
        let plain = encode_body(&der);
        let wrapped = plain
            .as_bytes()
            .chunks(64)
            .map(|c| String::from_utf8(c.to_vec()).unwrap())
            .collect::<Vec<_>>()
            .join("\r\n");
        assert_eq!(decode_body(wrapped.as_bytes()).unwrap(), der);
        assert_eq!(
            decode_body(format!("  {plain}\t\n").as_bytes()).unwrap(),
            der
        );
    }

    #[test]
    fn a_body_that_is_not_base64_is_nothing() {
        for bad in ["!!!!", "AAA", "not base64 at all"] {
            assert_eq!(decode_body(bad.as_bytes()), None, "{bad}");
        }
    }

    #[test]
    fn a_certs_only_message_is_a_signed_data_that_holds_the_certificates_and_nothing_else() {
        use cms::cert::x509::der::{Decode, Encode};
        use cms::content_info::ContentInfo;
        use cms::signed_data::SignedData;

        let make = || {
            rcgen::generate_simple_self_signed(vec!["x.local".to_owned()])
                .unwrap()
                .cert
                .der()
                .to_vec()
        };
        let (one, two) = (make(), make());
        let der = certs_only(&[&one, &two]);
        let info = ContentInfo::from_der(&der).unwrap();
        let signed: SignedData = info.content.decode_as().unwrap();
        assert_eq!(signed.digest_algorithms.len(), 0);
        assert!(signed.signer_infos.0.is_empty());
        assert!(signed.encap_content_info.econtent.is_none());
        let certificates = signed.certificates.unwrap().0;
        assert_eq!(certificates.len(), 2);
        let mut found: Vec<Vec<u8>> = certificates
            .iter()
            .map(|c| match c {
                cms::cert::CertificateChoices::Certificate(c) => c.to_der().unwrap(),
                other => panic!("{other:?}"),
            })
            .collect();
        found.sort();
        let mut expected = vec![one, two];
        expected.sort();
        assert_eq!(found, expected);
    }
}
