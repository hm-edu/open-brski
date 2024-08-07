//! [JSON Web Encryption](https://tools.ietf.org/html/rfc7516)
//!
//! This module contains code to implement JWE, the JOSE standard to encrypt arbitrary payloads.
//! Most commonly, JWE is used to encrypt a JWS payload, which is a signed JWT. For most common use,
//! you will want to look at the  [`Compact`](enum.Compact.html) enum.
use std::fmt;

use data_encoding::BASE64URL_NOPAD;

use serde::de::{self, DeserializeOwned};
use serde::{self, Deserialize, Deserializer, Serialize, Serializer};

use crate::biscuit::errors::{DecodeError, Error, ValidationError};
use crate::biscuit::jwa::{
    self, ContentEncryptionAlgorithm, EncryptionOptions, EncryptionResult, KeyManagementAlgorithm,
};
use crate::biscuit::jwk;
use crate::biscuit::{CompactJson, CompactPart, Empty};

#[derive(Debug, Eq, PartialEq, Clone)]
/// Compression algorithm applied to plaintext before encryption.
pub enum CompressionAlgorithm {
    /// DEFLATE algorithm defined in [RFC 1951](https://tools.ietf.org/html/rfc1951)
    Deflate,
    /// Other user-defined algorithm
    Other(String),
}

impl Serialize for CompressionAlgorithm {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let string = match *self {
            CompressionAlgorithm::Deflate => "DEF",
            CompressionAlgorithm::Other(ref other) => other,
        };

        serializer.serialize_str(string)
    }
}

impl<'de> Deserialize<'de> for CompressionAlgorithm {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct CompressionAlgorithmVisitor;
        impl<'de> de::Visitor<'de> for CompressionAlgorithmVisitor {
            type Value = CompressionAlgorithm;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "a string")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(match v {
                    "DEF" => CompressionAlgorithm::Deflate,
                    other => CompressionAlgorithm::Other(other.to_string()),
                })
            }
        }

        deserializer.deserialize_string(CompressionAlgorithmVisitor)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
/// Registered JWE header fields.
/// The fields are defined by [RFC 7516#4.1](https://tools.ietf.org/html/rfc7516#section-4.1)
pub struct RegisteredHeader {
    /// Algorithm used to encrypt or determine the value of the Content Encryption Key
    #[serde(rename = "alg")]
    pub cek_algorithm: KeyManagementAlgorithm,

    /// Content encryption algorithm used to perform authenticated encryption
    /// on the plaintext to produce the ciphertext and the Authentication Tag
    #[serde(rename = "enc")]
    pub enc_algorithm: ContentEncryptionAlgorithm,

    /// Compression algorithm applied to plaintext before encryption, if any.
    /// Compression is not supported at the moment.
    /// _Must only appear in integrity protected header._
    #[serde(rename = "zip", skip_serializing_if = "Option::is_none")]
    pub compression_algorithm: Option<CompressionAlgorithm>,

    /// Media type of the complete JWE. Serialized to `typ`.
    /// Defined in [RFC7519#5.1](https://tools.ietf.org/html/rfc7519#section-5.1) and additionally
    /// [RFC7515#4.1.9](https://tools.ietf.org/html/rfc7515#section-4.1.9).
    /// The "typ" value "JOSE" can be used by applications to indicate that
    /// this object is a JWS or JWE using the JWS Compact Serialization or
    /// the JWE Compact Serialization.  The "typ" value "JOSE+JSON" can be
    /// used by applications to indicate that this object is a JWS or JWE
    /// using the JWS JSON Serialization or the JWE JSON Serialization.
    /// Other type values can also be used by applications.
    #[serde(rename = "typ", skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,

    /// Content Type of the secured payload.
    /// Typically used to indicate the presence of a nested JOSE object which is signed or encrypted.
    /// Serialized to `cty`.
    /// Defined in [RFC7519#5.2](https://tools.ietf.org/html/rfc7519#section-5.2) and additionally
    /// [RFC7515#4.1.10](https://tools.ietf.org/html/rfc7515#section-4.1.10).
    #[serde(rename = "cty", skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,

    /// The JSON Web Key Set URL. This is currently not implemented (correctly).
    /// Serialized to `jku`.
    /// Defined in [RFC7515#4.1.2](https://tools.ietf.org/html/rfc7515#section-4.1.2).
    #[serde(rename = "jku", skip_serializing_if = "Option::is_none")]
    pub web_key_url: Option<String>,

    /// The JSON Web Key. This is currently not implemented (correctly).
    /// Serialized to `jwk`.
    /// Defined in [RFC7515#4.1.3](https://tools.ietf.org/html/rfc7515#section-4.1.3).
    #[serde(rename = "jwk", skip_serializing_if = "Option::is_none")]
    pub web_key: Option<String>,

    /// The Key ID. This is currently not implemented (correctly).
    /// Serialized to `kid`.
    /// Defined in [RFC7515#4.1.3](https://tools.ietf.org/html/rfc7515#section-4.1.3).
    #[serde(rename = "kid", skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,

    /// X.509 Public key cerfificate URL. This is currently not implemented (correctly).
    /// Serialized to `x5u`.
    /// Defined in [RFC7515#4.1.5](https://tools.ietf.org/html/rfc7515#section-4.1.5).
    #[serde(rename = "x5u", skip_serializing_if = "Option::is_none")]
    pub x509_url: Option<String>,

    /// X.509 public key certificate chain. This is currently not implemented (correctly).
    /// Serialized to `x5c`.
    /// Defined in [RFC7515#4.1.6](https://tools.ietf.org/html/rfc7515#section-4.1.6).
    #[serde(rename = "x5c", skip_serializing_if = "Option::is_none")]
    pub x509_chain: Option<Vec<String>>,

    /// X.509 Certificate thumbprint. This is currently not implemented (correctly).
    /// Also not implemented, is the SHA-256 thumbprint variant of this header.
    /// Serialized to `x5t`.
    /// Defined in [RFC7515#4.1.7](https://tools.ietf.org/html/rfc7515#section-4.1.7).
    // TODO: How to make sure the headers are mutually exclusive?
    #[serde(rename = "x5t", skip_serializing_if = "Option::is_none")]
    pub x509_fingerprint: Option<String>,

    /// List of critical extended headers.
    /// This is currently not implemented (correctly).
    /// Serialized to `crit`.
    /// Defined in [RFC7515#4.1.11](https://tools.ietf.org/html/rfc7515#section-4.1.11).
    #[serde(rename = "crit", skip_serializing_if = "Option::is_none")]
    pub critical: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
/// Headers specific to the Key management algorithm used. Users should typically not construct these fields as they
/// will be filled in automatically when encrypting and stripped when decrypting
pub struct CekAlgorithmHeader {
    /// Header for AES GCM Keywrap algorithm.
    /// The initialization vector, or nonce used in the encryption
    #[serde(rename = "iv", skip_serializing_if = "Option::is_none")]
    pub nonce: Option<Vec<u8>>,

    /// Header for AES GCM Keywrap algorithm.
    /// The authentication tag resulting from the encryption
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<Vec<u8>>,
}

/// JWE Header, consisting of the registered fields and other custom fields
#[derive(Debug, Eq, PartialEq, Clone, Default, Serialize, Deserialize)]
pub struct Header<T> {
    /// Registered header fields
    #[serde(flatten)]
    pub registered: RegisteredHeader,
    /// Key management algorithm specific headers
    #[serde(flatten)]
    pub cek_algorithm: CekAlgorithmHeader,
    /// Private header fields
    #[serde(flatten)]
    pub private: T,
}

impl<T: Serialize + DeserializeOwned> CompactJson for Header<T> {}

impl<T: Serialize + DeserializeOwned> Header<T> {
    /// Update CEK algorithm specific header fields based on a CEK encryption result
    fn update_cek_algorithm(&mut self, encrypted: &EncryptionResult) {
        if !encrypted.nonce.is_empty() {
            self.cek_algorithm.nonce = Some(encrypted.nonce.clone());
        }

        if !encrypted.tag.is_empty() {
            self.cek_algorithm.tag = Some(encrypted.tag.clone());
        }
    }

    /// Extract the relevant fields from the header to build an `EncryptionResult` and strip them from the header
    fn extract_cek_encryption_result(&mut self, encrypted_payload: &[u8]) -> EncryptionResult {
        let result = EncryptionResult {
            encrypted: encrypted_payload.to_vec(),
            nonce: self.cek_algorithm.nonce.clone().unwrap_or_default(),
            tag: self.cek_algorithm.tag.clone().unwrap_or_default(),
            ..Default::default()
        };

        self.cek_algorithm = Default::default();
        result
    }
}

impl Header<Empty> {
    /// Convenience function to create a header with only registered headers
    pub fn from_registered_header(registered: RegisteredHeader) -> Self {
        Self {
            registered,
            ..Default::default()
        }
    }
}

impl From<RegisteredHeader> for Header<Empty> {
    fn from(registered: RegisteredHeader) -> Self {
        Self::from_registered_header(registered)
    }
}

/// Compact representation of a JWE, or an encrypted JWT
///
/// This representation contains a payload of type `T` with custom headers provided by type `H`.
/// In general you should use a JWE with a JWS. That is, you should sign your JSON Web Token to
/// create a JWS, and then encrypt the signed JWS.
///
/// # Nonce/Initialization Vectors for AES GCM encryption
///
/// When encrypting tokens with AES GCM, you must take care _not to reuse_ the nonce for the same
/// key. You can keep track of this by simply treating the nonce as a 96 bit counter and
/// incrementing it every time you encrypt something new.
///
/// # Examples
/// ## Encrypting a JWS/JWT
/// See the example code in the [`biscuit::JWE`](../type.JWE.html) type alias.
///
/// ## Encrypting a string payload with A256GCMKW and A256GCM
/// ```
/// use std::str;
/// use biscuit::Empty;
/// use biscuit::jwk::JWK;
/// use biscuit::jwe;
/// use biscuit::jwa::{EncryptionOptions, KeyManagementAlgorithm, ContentEncryptionAlgorithm};
///
/// # #[allow(unused_assignments)]
/// # fn main() {
/// let payload = "The true sign of intelligence is not knowledge but imagination.";
/// // You would usually have your own AES key for this, but we will use a zeroed key as an example
/// let key: JWK<Empty> = JWK::new_octet_key(&vec![0; 256 / 8], Default::default());
///
/// // Construct the JWE
/// let jwe = jwe::Compact::new_decrypted(
///     From::from(jwe::RegisteredHeader {
///         cek_algorithm: KeyManagementAlgorithm::A256GCMKW,
///         enc_algorithm: ContentEncryptionAlgorithm::A256GCM,
///         ..Default::default()
///     }),
///     payload.as_bytes().to_vec(),
/// );
///
/// // We need to create an `EncryptionOptions` with a nonce for AES GCM encryption.
/// // You must take care NOT to reuse the nonce. You can simply treat the nonce as a 96 bit
/// // counter that is incremented after every use
/// let mut nonce_counter = num_bigint::BigUint::from_bytes_le(&vec![0; 96 / 8]);
/// // Make sure it's no more than 96 bits!
/// assert!(nonce_counter.bits() <= 96);
/// let mut nonce_bytes = nonce_counter.to_bytes_le();
/// // We need to ensure it is exactly 96 bits
/// nonce_bytes.resize(96/8, 0);
/// let options = EncryptionOptions::AES_GCM { nonce: nonce_bytes };
///
/// // Encrypt
/// let encrypted_jwe = jwe.encrypt(&key, &options).unwrap();
///
/// // Decrypt
/// let decrypted_jwe = encrypted_jwe
///     .decrypt(
///         &key,
///         KeyManagementAlgorithm::A256GCMKW,
///         ContentEncryptionAlgorithm::A256GCM,
///     )
///     .unwrap();
///
/// let decrypted_payload: &Vec<u8> = decrypted_jwe.payload().unwrap();
/// let decrypted_str = str::from_utf8(&*decrypted_payload).unwrap();
/// assert_eq!(decrypted_str, payload);
///
/// // Don't forget to increment the nonce!
/// nonce_counter = nonce_counter + 1u8;
/// # }
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Compact<T, H> {
    /// Decrypted form of the JWE.
    /// This variant cannot be serialized or deserialized and will return an error.
    #[serde(skip_serializing)]
    #[serde(skip_deserializing)]
    Decrypted {
        /// Embedded header
        header: Header<H>,
        /// Payload, usually a signed/unsigned JWT
        payload: T,
    },
    /// Encrypted JWT. Use this form to send to your clients
    Encrypted(crate::biscuit::Compact),
}

impl<T, H> Compact<T, H>
where
    T: CompactPart,
    H: Serialize + DeserializeOwned + Clone,
{
    /// Create a new encrypted JWE
    pub fn new_decrypted(header: Header<H>, payload: T) -> Self {
        Compact::Decrypted { header, payload }
    }

    /// Create a new encrypted JWE
    pub fn new_encrypted(token: &str) -> Self {
        Compact::Encrypted(crate::biscuit::Compact::decode(token))
    }

    /// Consumes self and encrypt it. If the token is already encrypted, this is a no-op.
    ///
    /// You will need to provide a `jwa::EncryptionOptions` that will differ based on your chosen
    /// algorithms.
    ///
    /// If your `cek_algorithm` is not `dir` or direct, the options provided will be used to
    /// encrypt your content encryption key.
    ///
    /// If your `cek_algorithm` is `dir` or Direct, then the options will be used to encrypt
    /// your content directly.
    pub fn into_encrypted<K: Serialize + DeserializeOwned>(
        self,
        key: &jwk::JWK<K>,
        options: &EncryptionOptions,
    ) -> Result<Self, Error> {
        match self {
            Compact::Encrypted(_) => Ok(self),
            Compact::Decrypted { .. } => self.encrypt(key, options),
        }
    }

    /// Encrypt an Decrypted JWE.
    ///
    /// You will need to provide a `jwa::EncryptionOptions` that will differ based on your chosen
    /// algorithms.
    ///
    /// If your `cek_algorithm` is not `dir` or direct, the options provided will be used to
    /// encrypt your content encryption key.
    ///
    /// If your `cek_algorithm` is `dir` or Direct, then the options will be used to encrypt
    /// your content directly.
    pub fn encrypt<K: Serialize + DeserializeOwned>(
        &self,
        key: &jwk::JWK<K>,
        options: &EncryptionOptions,
    ) -> Result<Self, Error> {
        match *self {
            Compact::Encrypted(_) => Err(Error::UnsupportedOperation),
            Compact::Decrypted {
                ref header,
                ref payload,
            } => {
                use std::borrow::Cow;

                // Resolve encryption option
                let (key_option, content_option): (_, Cow<'_, _>) =
                    match header.registered.cek_algorithm {
                        KeyManagementAlgorithm::DirectSymmetricKey => {
                            (jwa::NONE_ENCRYPTION_OPTIONS, Cow::Borrowed(options))
                        }
                        _ => (
                            options,
                            Cow::Owned(
                                header
                                    .registered
                                    .enc_algorithm
                                    .random_encryption_options()?,
                            ),
                        ),
                    };

                // RFC 7516 Section 5.1 describes the steps involved in encryption.
                // From steps 1 to 8, we will first determine the CEK, and then encrypt the CEK.
                let cek = header
                    .registered
                    .cek_algorithm
                    .cek(header.registered.enc_algorithm, key)?;
                let encrypted_cek = header.registered.cek_algorithm.wrap_key(
                    cek.algorithm.octet_key()?,
                    key,
                    key_option,
                )?;
                // Update header
                let mut header = header.clone();
                header.update_cek_algorithm(&encrypted_cek);

                // Steps 9 and 10 involves calculating an initialization vector (nonce) for content encryption. We do
                // this as part of the encryption process later

                // Step 11 involves compressing the payload, which we do not support at the moment
                let payload = payload.to_bytes()?;
                if header.registered.compression_algorithm.is_some() {
                    Err(Error::UnsupportedOperation)?
                }

                // Steps 12 to 14 involves the calculation of `Additional Authenticated Data` for encryption. In
                // our compact example, our header is the AAD.
                let encoded_protected_header = BASE64URL_NOPAD.encode(&header.to_bytes()?);
                // Step 15 involves the actual encryption.
                let encrypted_payload = header.registered.enc_algorithm.encrypt(
                    &payload,
                    encoded_protected_header.as_bytes(),
                    &cek,
                    &content_option,
                )?;

                // Finally create the JWE
                let mut compact = crate::biscuit::Compact::with_capacity(5);
                compact.push(&header)?;
                compact.push(&encrypted_cek.encrypted)?;
                compact.push(&encrypted_payload.nonce)?;
                compact.push(&encrypted_payload.encrypted)?;
                compact.push(&encrypted_payload.tag)?;

                Ok(Compact::Encrypted(compact))
            }
        }
    }

    /// Consumes self and decrypt it. If the token is already decrypted,
    /// this is a no-op.
    pub fn into_decrypted<K: Serialize + DeserializeOwned>(
        self,
        key: &jwk::JWK<K>,
        cek_alg: KeyManagementAlgorithm,
        enc_alg: ContentEncryptionAlgorithm,
    ) -> Result<Self, Error> {
        match self {
            Compact::Encrypted(_) => self.decrypt(key, cek_alg, enc_alg),
            Compact::Decrypted { .. } => Ok(self),
        }
    }

    /// Decrypt an encrypted JWE. Provide the expected algorithms to mitigate an attacker modifying the
    /// fields
    pub fn decrypt<K: Serialize + DeserializeOwned>(
        &self,
        key: &jwk::JWK<K>,
        cek_alg: KeyManagementAlgorithm,
        enc_alg: ContentEncryptionAlgorithm,
    ) -> Result<Self, Error> {
        match *self {
            Compact::Encrypted(ref encrypted) => {
                if encrypted.len() != 5 {
                    Err(DecodeError::PartsLengthError {
                        actual: encrypted.len(),
                        expected: 5,
                    })?
                }
                // RFC 7516 Section 5.2 describes the steps involved in decryption.
                // Steps 1-3
                let mut header: Header<H> = encrypted.part(0)?;
                let encrypted_cek: Vec<u8> = encrypted.part(1)?;
                let nonce: Vec<u8> = encrypted.part(2)?;
                let encrypted_payload: Vec<u8> = encrypted.part(3)?;
                let tag: Vec<u8> = encrypted.part(4)?;

                // Verify that the algorithms are expected
                if header.registered.cek_algorithm != cek_alg
                    || header.registered.enc_algorithm != enc_alg
                {
                    Err(Error::ValidationError(
                        ValidationError::WrongAlgorithmHeader,
                    ))?;
                }

                // TODO: Steps 4-5 not implemented at the moment.

                // Steps 6-13 involve the computation of the cek
                let cek_encryption_result = header.extract_cek_encryption_result(&encrypted_cek);
                let cek = header.registered.cek_algorithm.unwrap_key(
                    &cek_encryption_result,
                    header.registered.enc_algorithm,
                    key,
                )?;

                // Build encryption result as per steps 14-15
                let protected_header: Vec<u8> = encrypted.part(0)?;
                let encoded_protected_header = BASE64URL_NOPAD.encode(protected_header.as_ref());
                let encrypted_payload_result = EncryptionResult {
                    nonce,
                    tag,
                    encrypted: encrypted_payload,
                    additional_data: encoded_protected_header.as_bytes().to_vec(),
                };

                let payload = header
                    .registered
                    .enc_algorithm
                    .decrypt(&encrypted_payload_result, &cek)?;

                // Decompression is not supported at the moment
                if header.registered.compression_algorithm.is_some() {
                    Err(Error::UnsupportedOperation)?
                }

                let payload = T::from_bytes(&payload)?;

                Ok(Compact::new_decrypted(header, payload))
            }
            Compact::Decrypted { .. } => Err(Error::UnsupportedOperation),
        }
    }

    /// Convenience method to get a reference to the encrypted payload
    pub fn encrypted(&self) -> Result<&crate::biscuit::Compact, Error> {
        match *self {
            Compact::Decrypted { .. } => Err(Error::UnsupportedOperation),
            Compact::Encrypted(ref encoded) => Ok(encoded),
        }
    }

    /// Convenience method to get a mutable reference to the encrypted payload
    pub fn encrypted_mut(&mut self) -> Result<&mut crate::biscuit::Compact, Error> {
        match *self {
            Compact::Decrypted { .. } => Err(Error::UnsupportedOperation),
            Compact::Encrypted(ref mut encoded) => Ok(encoded),
        }
    }

    /// Convenience method to get a reference to the payload from an Decrypted JWE
    pub fn payload(&self) -> Result<&T, Error> {
        match *self {
            Compact::Decrypted { ref payload, .. } => Ok(payload),
            Compact::Encrypted(_) => Err(Error::UnsupportedOperation),
        }
    }

    /// Convenience method to get a mutable reference to the payload from an Decrypted JWE
    pub fn payload_mut(&mut self) -> Result<&mut T, Error> {
        match *self {
            Compact::Decrypted {
                ref mut payload, ..
            } => Ok(payload),
            Compact::Encrypted(_) => Err(Error::UnsupportedOperation),
        }
    }

    /// Convenience method to get a reference to the header from an Decrypted JWE
    pub fn header(&self) -> Result<&Header<H>, Error> {
        match *self {
            Compact::Decrypted { ref header, .. } => Ok(header),
            Compact::Encrypted(_) => Err(Error::UnsupportedOperation),
        }
    }

    /// Convenience method to get a reference to the header from an Decrypted JWE
    pub fn header_mut(&mut self) -> Result<&mut Header<H>, Error> {
        match *self {
            Compact::Decrypted { ref mut header, .. } => Ok(header),
            Compact::Encrypted(_) => Err(Error::UnsupportedOperation),
        }
    }

    /// Consumes self, and move the payload and header out and return them as a tuple
    ///
    /// # Panics
    /// Panics if the JWE is not decrypted
    pub fn unwrap_decrypted(self) -> (Header<H>, T) {
        match self {
            Compact::Decrypted { header, payload } => (header, payload),
            Compact::Encrypted(_) => panic!("JWE is encrypted"),
        }
    }

    /// Consumes self, and move the encrypted Compact serialization out and return it
    ///
    /// # Panics
    /// Panics if the JWE is not encrypted
    pub fn unwrap_encrypted(self) -> crate::biscuit::Compact {
        match self {
            Compact::Decrypted { .. } => panic!("JWE is decrypted"),
            Compact::Encrypted(compact) => compact,
        }
    }
}

/// Convenience implementation for a Compact that contains a `ClaimsSet`
impl<P, H> Compact<crate::biscuit::ClaimsSet<P>, H>
where
    crate::biscuit::ClaimsSet<P>: CompactPart,
    H: Serialize + DeserializeOwned + Clone,
{
    /// Validate the temporal claims in the decoded token
    ///
    /// If `None` is provided for options, the defaults will apply.
    ///
    /// By default, no temporal claims (namely `iat`, `exp`, `nbf`)
    /// are required, and they will pass validation if they are missing.
    pub fn validate(&self, options: crate::biscuit::ValidationOptions) -> Result<(), Error> {
        self.payload()?.registered.validate(options)?;
        Ok(())
    }
}
