//! 标准 Noise XX 握手。中转不能派生业务密钥；身份核对依赖用户比较双方本地摘要代码。
use assemble_protocol::remote::secure::{Binding, Channel, MAX_PLAIN_BYTES, Sealed};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use zeroize::Zeroizing;
const CHUNK: usize = 60 * 1024;
#[derive(Debug, thiserror::Error)]
#[error("remote secure channel failed")]
pub struct CryptoError;
pub struct Initiator(snow::HandshakeState);
pub struct Responder(snow::HandshakeState);
pub struct Cipher {
    state: snow::StatelessTransportState,
    binding: Binding,
    nonce: u64,
    code: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Message<T> {
    binding: Binding,
    channel: Channel,
    sequence: u32,
    value: T,
}
fn handshake(host: &str, initiator: bool) -> Result<snow::HandshakeState, CryptoError> {
    let params = "Noise_XX_25519_ChaChaPoly_SHA256"
        .parse()
        .map_err(|_| CryptoError)?;
    let builder = snow::Builder::new(params);
    let keys = builder.generate_keypair().map_err(|_| CryptoError)?;
    let private = Zeroizing::new(keys.private);
    let prologue = serde_json::to_vec(&("AssembleRemote/2", host)).map_err(|_| CryptoError)?;
    let builder = builder
        .prologue(&prologue)
        .map_err(|_| CryptoError)?
        .local_private_key(&private)
        .map_err(|_| CryptoError)?;
    if initiator {
        builder.build_initiator()
    } else {
        builder.build_responder()
    }
    .map_err(|_| CryptoError)
}
fn write(state: &mut snow::HandshakeState, value: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let mut output = vec![0; 2048];
    let length = state
        .write_message(value, &mut output)
        .map_err(|_| CryptoError)?;
    output.truncate(length);
    Ok(output)
}
fn read(
    state: &mut snow::HandshakeState,
    packet: &[u8],
    expected: &[u8],
) -> Result<(), CryptoError> {
    if packet.len() > 2048 {
        return Err(CryptoError);
    }
    let mut output = [0; 2048];
    let len = state
        .read_message(packet, &mut output)
        .map_err(|_| CryptoError)?;
    if &output[..len] != expected {
        return Err(CryptoError);
    }
    Ok(())
}
fn transport(state: snow::HandshakeState, binding: Binding) -> Result<Cipher, CryptoError> {
    let hash = state.get_handshake_hash();
    let hex = assemble_protocol::primitives::to_hex(hash.get(..12).ok_or(CryptoError)?);
    let code = hex
        .as_bytes()
        .chunks(4)
        .map(|part| std::str::from_utf8(part).map(str::to_owned))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CryptoError)?
        .join("-");
    Ok(Cipher {
        state: state
            .into_stateless_transport_mode()
            .map_err(|_| CryptoError)?,
        binding,
        nonce: 0,
        code,
    })
}
impl Initiator {
    pub fn start(host: &str) -> Result<(Self, Vec<u8>), CryptoError> {
        let mut state = handshake(host, true)?;
        let offer = write(&mut state, &[])?;
        Ok((Self(state), offer))
    }
    pub fn finish(
        mut self,
        binding: Binding,
        response: &[u8],
    ) -> Result<(Cipher, Vec<u8>), CryptoError> {
        let payload = serde_json::to_vec(&binding).map_err(|_| CryptoError)?;
        read(&mut self.0, response, &payload)?;
        let finish = write(&mut self.0, &payload)?;
        Ok((transport(self.0, binding)?, finish))
    }
}
impl Responder {
    pub fn start(binding: &Binding, offer: &[u8]) -> Result<(Self, Vec<u8>), CryptoError> {
        let mut state = handshake(&binding.host_id, false)?;
        read(&mut state, offer, &[])?;
        let payload = serde_json::to_vec(binding).map_err(|_| CryptoError)?;
        let response = write(&mut state, &payload)?;
        Ok((Self(state), response))
    }
    pub fn finish(mut self, binding: Binding, finish: &[u8]) -> Result<Cipher, CryptoError> {
        read(
            &mut self.0,
            finish,
            &serde_json::to_vec(&binding).map_err(|_| CryptoError)?,
        )?;
        transport(self.0, binding)
    }
}
impl Cipher {
    pub fn code(&self) -> &str {
        &self.code
    }
    pub fn seal<T: Serialize>(
        &mut self,
        channel: Channel,
        sequence: u32,
        value: &T,
    ) -> Result<Sealed, CryptoError> {
        let plain = Zeroizing::new(
            serde_json::to_vec(&Message {
                binding: self.binding.clone(),
                channel,
                sequence,
                value,
            })
            .map_err(|_| CryptoError)?,
        );
        if plain.len() > MAX_PLAIN_BYTES {
            return Err(CryptoError);
        }
        let nonce = self.nonce;
        self.nonce = self
            .nonce
            .checked_add(u64::try_from(plain.len().div_ceil(CHUNK)).map_err(|_| CryptoError)?)
            .ok_or(CryptoError)?;
        let mut chunks = Vec::new();
        for (index, part) in plain.chunks(CHUNK).enumerate() {
            let mut bytes = vec![0; part.len() + 16];
            let len = self
                .state
                .write_message(
                    nonce + u64::try_from(index).map_err(|_| CryptoError)?,
                    part,
                    &mut bytes,
                )
                .map_err(|_| CryptoError)?;
            chunks.push(STANDARD.encode(&bytes[..len]));
        }
        Ok(Sealed { nonce, chunks })
    }
    pub fn open<T: DeserializeOwned>(
        &self,
        packet: &Sealed,
        channel: Channel,
        sequence: u32,
    ) -> Result<T, CryptoError> {
        if !packet.valid() {
            return Err(CryptoError);
        }
        let mut plain = Zeroizing::new(Vec::new());
        for (index, chunk) in packet.chunks.iter().enumerate() {
            let bytes = STANDARD.decode(chunk).map_err(|_| CryptoError)?;
            if bytes.len() > CHUNK + 16 {
                return Err(CryptoError);
            }
            let mut output = Zeroizing::new(vec![0; CHUNK]);
            let nonce = packet
                .nonce
                .checked_add(u64::try_from(index).map_err(|_| CryptoError)?)
                .ok_or(CryptoError)?;
            let len = self
                .state
                .read_message(nonce, &bytes, &mut output)
                .map_err(|_| CryptoError)?;
            plain.extend_from_slice(&output[..len]);
            if plain.len() > MAX_PLAIN_BYTES {
                return Err(CryptoError);
            }
        }
        let result: Message<T> = serde_json::from_slice(&plain).map_err(|_| CryptoError)?;
        if result.binding != self.binding
            || result.channel != channel
            || result.sequence != sequence
        {
            return Err(CryptoError);
        }
        Ok(result.value)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn pair() -> (Cipher, Cipher) {
        let binding = Binding {
            session_id: "session".into(),
            controller_id: "controller".into(),
            host_id: "host".into(),
        };
        let (initiator, offer) = Initiator::start("host").unwrap();
        let (responder, response) = Responder::start(&binding, &offer).unwrap();
        let (controller, finish) = initiator.finish(binding.clone(), &response).unwrap();
        let host = responder.finish(binding, &finish).unwrap();
        (controller, host)
    }
    #[test]
    fn authenticated_chunks_bind_session_direction_type_and_sequence() {
        let (mut controller, host) = pair();
        assert_eq!(controller.code(), host.code());
        assert_eq!(controller.code().len(), 29);
        let value = "private".repeat(25_000);
        let packet = controller.seal(Channel::Frame, 1, &value).unwrap();
        assert!(packet.chunks.len() > 1);
        assert_eq!(
            host.open::<String>(&packet, Channel::Frame, 1).unwrap(),
            value
        );
        assert!(host.open::<String>(&packet, Channel::Input, 1).is_err());
        assert!(host.open::<String>(&packet, Channel::Frame, 2).is_err());
        assert!(
            controller
                .open::<String>(&packet, Channel::Frame, 1)
                .is_err()
        );
        let (_, other) = pair();
        assert!(other.open::<String>(&packet, Channel::Frame, 1).is_err());
        let mut truncated = packet.clone();
        truncated.chunks.pop();
        assert!(host.open::<String>(&truncated, Channel::Frame, 1).is_err());
        let mut changed = packet.clone();
        let replacement = if changed.chunks[0].starts_with('A') {
            "B"
        } else {
            "A"
        };
        changed.chunks[0].replace_range(..1, replacement);
        assert!(host.open::<String>(&changed, Channel::Frame, 1).is_err());
        let mut tampered = packet;
        tampered.nonce += 1;
        assert!(host.open::<String>(&tampered, Channel::Frame, 1).is_err());
    }
}
