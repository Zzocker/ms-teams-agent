//! Translate audio SDP between GStreamer's WebRTC transport and the Teams calling service.

use gstreamer_sdp::{SDPAttribute, SDPConnection, SDPMedia, SDPMediaRef, SDPMessage};
use std::net::IpAddr;

const TEAMS_PROFILE: &str = "RTP/SAVP";
const WEBRTC_PROFILE: &str = "UDP/TLS/RTP/SAVPF";
const TCP_TYPES: [(&str, &str); 3] = [
    ("active", "tcp-act"),
    ("passive", "tcp-pass"),
    ("so", "tcp-so"),
];

pub struct AudioOffer {
    pub wire: String,
    pub local_mid: String,
}

pub fn to_teams(web_rtc: &str) -> Result<AudioOffer, String> {
    let mut sdp = parse(web_rtc)?;
    let fingerprint = sdp.attribute_val("fingerprint").map(str::to_owned);
    let audio = sdp
        .medias_mut()
        .next()
        .ok_or("The local offer is missing audio.")?;
    if audio.media() != Some("audio") || audio.proto() != Some(WEBRTC_PROFILE) {
        return Err("The local offer is missing DTLS audio.".into());
    }
    let local_mid = audio
        .attribute_val("mid")
        .ok_or("The local audio stream has no media ID.")?
        .to_owned();
    remove_audio_attribute(audio, "mid")?;
    audio.add_attribute("mid", Some("0"));
    audio.set_proto(TEAMS_PROFILE);
    audio.add_attribute("label", Some("main-audio"));
    audio.add_attribute("x-signaling-fb", Some("* x-message app recv:dsh"));
    if audio.attribute_val("fingerprint").is_none()
        && let Some(fingerprint) = fingerprint
    {
        audio.add_attribute("fingerprint", Some(&fingerprint));
    }
    let ssrc = audio
        .attribute_val("ssrc")
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse::<u32>().ok());
    if let Some(ssrc) = ssrc {
        audio.add_attribute("x-ssrc-range", Some(&format!("{ssrc}-{ssrc}")));
    }
    if let Some((ip, port)) = audio
        .attributes()
        .filter(|attribute| attribute.key() == "candidate")
        .filter_map(|attribute| {
            let mut fields = attribute.value()?.split_whitespace();
            fields.next()?;
            if fields.next()? != "1" || !fields.next()?.eq_ignore_ascii_case("udp") {
                return None;
            }
            let priority = fields.next()?.parse::<u32>().ok()?;
            let ip = fields.next()?.parse::<IpAddr>().ok()?;
            let port = fields
                .next()?
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)?;
            let relay = fields.next() == Some("typ") && fields.next() == Some("relay");
            Some((relay, priority, ip, port))
        })
        .max_by_key(|(relay, priority, _, _)| (*relay, *priority))
        .map(|(_, _, ip, port)| (ip, port))
    {
        audio.set_port_info(u32::from(port), 0);
        let family = if ip.is_ipv4() { "IP4" } else { "IP6" };
        let address = ip.to_string();
        if audio.connections_len() == 0 {
            audio.add_connection("IN", family, &address, 0, 0);
        } else {
            audio
                .replace_connection(0, SDPConnection::new("IN", family, &address, 0, 0))
                .map_err(|error| error.to_string())?;
        }
        remove_audio_attribute(audio, "rtcp")?;
        audio.add_attribute("rtcp", Some(&port.to_string()));
    }
    rewrite_candidates(audio, true)?;
    remove_audio_attribute(audio, "end-of-candidates")?;
    remove_audio_attribute(audio, "rtcp-mux-only")?;
    remove_audio_attribute(audio, "rtcp-rsize")?;
    let audio_port = audio.port();
    let connection = audio.connection(0).cloned();
    let transport: Vec<_> = ["ice-ufrag", "ice-pwd", "fingerprint", "setup"]
        .into_iter()
        .filter_map(|name| {
            audio
                .attribute_val(name)
                .map(|value| (name, value.to_owned()))
        })
        .collect();
    remove_session_attribute(&mut sdp, "fingerprint")?;
    remove_session_attribute(&mut sdp, "msid-semantic")?;
    sdp.add_attribute("msid-semantic", Some("WMS *"));
    remove_session_attribute(&mut sdp, "group")?;
    sdp.add_attribute("group", Some("BUNDLE 0 1 2 3"));
    // Keep the remaining Teams Web sections on the same transport with no media enabled.
    for (kind, mid, label) in [
        ("video", "1", "main-video"),
        ("video", "2", "applicationsharing-video"),
        ("x-data", "3", "data"),
    ] {
        let mut disabled = SDPMedia::new();
        disabled.set_media(kind);
        disabled.set_port_info(audio_port, 0);
        disabled.set_proto(TEAMS_PROFILE);
        if kind == "video" {
            disabled.add_format("126");
            disabled.add_attribute("rtpmap", Some("126 H264/90000"));
            disabled.add_attribute(
                "fmtp",
                Some("126 profile-level-id=42e01f;level-asymmetry-allowed=1;packetization-mode=1"),
            );
        } else {
            disabled.add_format("127");
            disabled.add_attribute("rtpmap", Some("127 x-data/90000"));
            disabled.add_attribute("x-data-protocol", Some("sctp"));
            disabled.add_attribute("sctp-port", Some("5000"));
        }
        if let Some(connection) = &connection {
            disabled
                .insert_connection(None, connection.clone())
                .map_err(|error| error.to_string())?;
        }
        for (key, value) in &transport {
            disabled.add_attribute(key, Some(value));
        }
        disabled.add_attribute("mid", Some(mid));
        disabled.add_attribute("label", Some(label));
        disabled.add_attribute("inactive", None);
        disabled.add_attribute("rtcp-mux", None);
        disabled.add_attribute("bundle-only", None);
        sdp.add_media(disabled);
    }
    let wire = sdp.as_text().map_err(|error| error.to_string())?;
    Ok(AudioOffer { wire, local_mid })
}

pub fn from_teams(teams_sdp: &str, local_mid: &str) -> Result<String, String> {
    let mut audio_sdp = String::new();
    let mut include = true;
    for line in teams_sdp.lines() {
        if line.starts_with("m=") {
            include = line.starts_with("m=audio ");
        }
        if include {
            audio_sdp.push_str(line);
            audio_sdp.push_str("\r\n");
        }
    }
    let mut parsed = parse(&audio_sdp)?;
    let has_session_fingerprint = parsed.attribute_val("fingerprint").is_some();
    let directions = ["sendrecv", "sendonly", "recvonly", "inactive"];
    let session_direction = directions
        .into_iter()
        .find(|key| parsed.attributes().any(|attr| attr.key() == *key));
    let audio = parsed
        .medias_mut()
        .next()
        .ok_or("The Teams answer is missing audio.")?;
    if audio.media() != Some("audio") || audio.port() == 0 {
        return Err("Teams did not accept the audio stream.".into());
    }
    if audio.attribute_val("fingerprint").is_none() && !has_session_fingerprint {
        return Err("The Teams audio answer is missing a DTLS fingerprint.".into());
    }
    if !audio
        .attributes()
        .any(|attr| directions.contains(&attr.key()))
    {
        // An omitted SDP direction defaults to sendrecv.
        audio.add_attribute(session_direction.unwrap_or("sendrecv"), None);
    }
    audio.set_proto(WEBRTC_PROFILE);
    remove_audio_attribute(audio, "mid")?;
    audio.add_attribute("mid", Some(local_mid));
    rewrite_candidates(audio, false)?;
    remove_audio_attribute(audio, "crypto")?;
    remove_session_attribute(&mut parsed, "group")?;
    parsed.add_attribute("group", Some(&format!("BUNDLE {local_mid}")));
    parsed.as_text().map_err(|error| error.to_string())
}

fn parse(sdp: &str) -> Result<SDPMessage, String> {
    SDPMessage::parse_buffer(sdp.as_bytes()).map_err(|error| error.to_string())
}

fn remove_audio_attribute(audio: &mut SDPMediaRef, key: &str) -> Result<(), String> {
    let indexes: Vec<_> = (0..audio.attributes_len())
        .filter(|index| {
            audio
                .attribute(*index)
                .is_some_and(|attribute| attribute.key() == key)
        })
        .collect();
    for index in indexes.into_iter().rev() {
        audio
            .remove_attribute(index)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn remove_session_attribute(sdp: &mut SDPMessage, key: &str) -> Result<(), String> {
    let indexes: Vec<_> = (0..sdp.attributes_len())
        .filter(|index| {
            sdp.attribute(*index)
                .is_some_and(|attribute| attribute.key() == key)
        })
        .collect();
    for index in indexes.into_iter().rev() {
        sdp.remove_attribute(index)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn rewrite_candidates(audio: &mut SDPMediaRef, to_teams: bool) -> Result<(), String> {
    for index in 0..audio.attributes_len() {
        let Some(attribute) = audio.attribute(index) else {
            continue;
        };
        if attribute.key() != "candidate" && (to_teams || attribute.key() != "x-candidate-ipv6") {
            continue;
        }
        let mut fields: Vec<_> = attribute
            .value()
            .unwrap_or_default()
            .split_whitespace()
            .collect();
        if fields.len() < 8 {
            continue;
        }
        if to_teams {
            if fields[2].eq_ignore_ascii_case("tcp")
                && let Some(index) = (8..fields.len() - 1)
                    .step_by(2)
                    .find(|i| fields[*i] == "tcptype")
                && let Some((_, wire)) = TCP_TYPES
                    .iter()
                    .find(|(kind, _)| *kind == fields[index + 1])
            {
                fields[2] = wire;
                fields.drain(index..index + 2);
            }
            for index in (8..fields.len().saturating_sub(1)).step_by(2).rev() {
                if matches!(fields[index], "generation" | "network-id" | "network-cost") {
                    fields.drain(index..index + 2);
                }
            }
        } else if let Some((kind, _)) = TCP_TYPES
            .iter()
            .find(|(_, wire)| wire.eq_ignore_ascii_case(fields[2]))
        {
            fields[2] = "TCP";
            fields.extend_from_slice(&["tcptype", kind]);
        }
        audio
            .replace_attribute(
                index,
                SDPAttribute::new("candidate", Some(&fields.join(" "))),
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEBRTC_OFFER: &str = concat!(
        "v=0\r\n",
        "o=- 1 1 IN IP4 127.0.0.1\r\n",
        "s=-\r\n",
        "t=0 0\r\n",
        "a=group:BUNDLE audio0\r\n",
        "a=fingerprint:sha-256 AA:BB\r\n",
        "m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n",
        "c=IN IP4 0.0.0.0\r\n",
        "a=mid:audio0\r\n",
        "a=setup:actpass\r\n",
        "a=rtcp-mux\r\n",
        "a=rtpmap:111 OPUS/48000/2\r\n",
        "a=ssrc:42 cname:audio\r\n",
        "a=candidate:1 1 UDP 100 192.0.2.10 50000 typ host generation 0\r\n",
        "a=candidate:2 1 TCP 90 192.0.2.10 9 typ host tcptype active generation 0\r\n",
        "a=end-of-candidates\r\n"
    );

    #[test]
    fn writes_the_teams_audio_profile_and_preserves_dtls() {
        gstreamer::init().unwrap();
        let wire = parse(&to_teams(WEBRTC_OFFER).unwrap().wire).unwrap();
        let audio = wire.media(0).unwrap();
        assert_eq!(audio.proto(), Some(TEAMS_PROFILE));
        assert_eq!(audio.attribute_val("mid"), Some("0"));
        assert_eq!(wire.medias().count(), 4);
        assert!(wire.medias().skip(1).all(|media| {
            media.attributes().any(|attr| attr.key() == "inactive")
                && media.attributes().any(|attr| attr.key() == "bundle-only")
        }));
        assert_eq!(audio.port(), 50_000);
        assert_eq!(audio.attribute_val("label"), Some("main-audio"));
        assert_eq!(audio.attribute_val("x-ssrc-range"), Some("42-42"));
        assert_eq!(audio.attribute_val("fingerprint"), Some("sha-256 AA:BB"));
        assert_eq!(
            wire.attribute_val("msid-semantic").map(str::trim),
            Some("WMS *")
        );
        assert!(
            audio
                .attributes()
                .any(|attr| attr.value().is_some_and(|value| value.contains("tcp-act")))
        );
        assert!(!audio.attributes().any(|attr| {
            attr.value()
                .is_some_and(|value| value.contains("generation"))
        }));
    }

    #[test]
    fn restores_the_webrtc_profile_and_standard_tcp_candidates() {
        gstreamer::init().unwrap();
        let offer = to_teams(WEBRTC_OFFER).unwrap();
        let wire = &offer.wire;
        let converted = parse(&from_teams(wire, &offer.local_mid).unwrap()).unwrap();
        let audio = converted.media(0).unwrap();
        assert_eq!(audio.proto(), Some(WEBRTC_PROFILE));
        assert_eq!(audio.attribute_val("mid"), Some("audio0"));
        assert_eq!(converted.medias().count(), 1);
        assert_eq!(audio.attribute_val("fingerprint"), Some("sha-256 AA:BB"));
        assert_eq!(audio.attribute_val("setup"), Some("actpass"));
        assert!(audio.attributes().any(|attr| attr.key() == "sendrecv"));
        assert!(audio.attributes().any(|attr| {
            attr.value()
                .is_some_and(|value| value.ends_with("tcptype active"))
        }));
        let missing_fingerprint = wire
            .lines()
            .filter(|line| !line.starts_with("a=fingerprint:"))
            .collect::<Vec<_>>()
            .join("\r\n");
        assert!(from_teams(&missing_fingerprint, &offer.local_mid).is_err());
    }

    #[test]
    fn preserves_media_direction_and_inherits_session_direction_when_omitted() {
        gstreamer::init().unwrap();
        let offer = to_teams(WEBRTC_OFFER).unwrap();
        let session_direction = offer.wire.replace("m=audio ", "a=sendonly\r\nm=audio ");
        let media_direction =
            session_direction.replace("a=setup:actpass", "a=recvonly\r\na=setup:actpass");
        for (answer, expected) in [
            (session_direction, "sendonly"),
            (media_direction, "recvonly"),
        ] {
            let converted = parse(&from_teams(&answer, &offer.local_mid).unwrap()).unwrap();
            let audio = converted.media(0).unwrap();
            assert!(audio.attributes().any(|attr| attr.key() == expected));
        }
    }
}
