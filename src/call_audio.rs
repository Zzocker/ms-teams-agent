//! Native Opus audio and WebRTC transport for a single call.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_sdp::SDPMessage;
use gstreamer_webrtc::{
    WebRTCICEGatheringState, WebRTCPeerConnectionState, WebRTCSDPType, WebRTCSessionDescription,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

pub struct AudioPeer {
    pipeline: gst::Pipeline,
    peer: gst::Element,
}

const TEAMS_RINGBACK: &[u8] = include_bytes!("../assets/audio/teams-outgoing-ringback.mp3");

/// Keep the local cue independent of network setup so cancellation can stop it promptly.
pub struct Ringback {
    _stop: mpsc::Sender<()>,
}

struct RingbackPlayback(gst::Pipeline);

impl Ringback {
    pub fn start(cancelled: Arc<AtomicBool>) -> Self {
        let (stop, listen) = mpsc::channel();
        std::thread::spawn(move || {
            let keep_playing = || {
                !cancelled.load(Ordering::Relaxed)
                    && matches!(listen.try_recv(), Err(mpsc::TryRecvError::Empty))
            };
            if !keep_playing() || gst::init().is_err() {
                return;
            }
            let mut tone = RingbackPlayback::bundled();
            let mut official = true;
            while keep_playing() {
                if official && tone.as_ref().is_none_or(|tone| !tone.playing()) {
                    drop(tone.take());
                    tone = RingbackPlayback::fallback();
                    official = false;
                    if tone.is_none() {
                        return;
                    }
                }
                if listen.recv_timeout(Duration::from_millis(25))
                    != Err(mpsc::RecvTimeoutError::Timeout)
                {
                    return;
                }
            }
        });
        Self { _stop: stop }
    }
}

impl RingbackPlayback {
    fn bundled() -> Option<Self> {
        // GStreamer's data URI source reads and seeks within the embedded MP3.
        let uri = format!("data:audio/mpeg;base64,{}", STANDARD.encode(TEAMS_RINGBACK));
        let pipeline = gst::ElementFactory::make("playbin")
            .property("uri", uri)
            .build()
            .ok()?
            .downcast::<gst::Pipeline>()
            .ok()?;
        Self::start(pipeline)
    }

    fn fallback() -> Option<Self> {
        // Layer two softly ramped notes into a short chime that repeats every 2.8s.
        let pipeline = gst::parse::launch(
            "audiomixer name=mix ! audioconvert ! audioresample ! autoaudiosink
             audiotestsrc is-live=true wave=ticks freq=440 volume=0.10
               apply-tick-ramp=true sine-periods-per-tick=76 tick-interval=2800000000
               ! audio/x-raw,rate=48000,channels=1 ! mix.
             audiotestsrc is-live=true wave=ticks freq=587 volume=0.085
               apply-tick-ramp=true sine-periods-per-tick=117 tick-interval=2800000000
               ! audio/x-raw,rate=48000,channels=1 ! mix.",
        )
        .ok()?
        .downcast::<gst::Pipeline>()
        .ok()?;
        Self::start(pipeline)
    }

    fn start(pipeline: gst::Pipeline) -> Option<Self> {
        let tone = Self(pipeline);
        tone.0.set_state(gst::State::Playing).ok()?;
        Some(tone)
    }

    fn playing(&self) -> bool {
        let Some(bus) = self.0.bus() else {
            return false;
        };
        match bus.pop_filtered(&[gst::MessageType::Error, gst::MessageType::Eos]) {
            Some(message) if message.type_() == gst::MessageType::Error => false,
            Some(_) => self
                .0
                .seek_simple(gst::SeekFlags::FLUSH, gst::ClockTime::ZERO)
                .is_ok(),
            None => true,
        }
    }
}

impl Drop for RingbackPlayback {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}

/// Share the outgoing stream's mute control between the UI and the audio worker.
#[derive(Clone, Default)]
pub struct Microphone(Arc<Mutex<MicrophoneState>>);

#[derive(Default)]
struct MicrophoneState {
    muted: bool,
    volume: Option<gst::Element>,
}

impl Microphone {
    pub fn set_muted(&self, muted: bool) {
        let mut state = self.0.lock().unwrap();
        state.muted = muted;
        if let Some(volume) = &state.volume {
            volume.set_property("mute", muted);
        }
    }

    fn attach(&self, volume: gst::Element) {
        let mut state = self.0.lock().unwrap();
        volume.set_property("mute", state.muted);
        state.volume = Some(volume);
    }
}

impl AudioPeer {
    pub fn start(relays: &[String], microphone: &Microphone) -> Result<Self, String> {
        gst::init().map_err(|error| error.to_string())?;
        let ssrc = (uuid::Uuid::new_v4().as_u128() as u32).clamp(1, u32::MAX - 1);
        // Advertise the RTP stream at offer time, before the microphone delivers samples.
        // RFC 7587 requires the Opus channel count in SDP to be 2, including mono.
        let pipeline = gst::parse::launch(&format!(
            "webrtcbin name=peer bundle-policy=max-bundle
             autoaudiosrc ! audioconvert ! audioresample ! volume name=microphone ! opusenc ! rtpopuspay pt=111 ssrc={ssrc}
             ! application/x-rtp,media=audio,encoding-name=OPUS,encoding-params=(string)2,clock-rate=48000,payload=111,ssrc=(uint){ssrc} ! peer.
             queue name=receive ! rtpopusdepay ! opusdec ! audioconvert ! audioresample ! autoaudiosink"
        )).map_err(|error| error.to_string())?.downcast::<gst::Pipeline>()
            .map_err(|_| "Could not create the WebRTC audio pipeline.")?;
        let peer = pipeline
            .by_name("peer")
            .ok_or("Missing WebRTC audio element.")?;
        let mut added_relay = false;
        for relay in relays {
            added_relay |= peer.emit_by_name::<bool>("add-turn-server", &[&relay]);
        }
        if !relays.is_empty() && !added_relay {
            return Err("Could not configure the Teams audio relay.".into());
        }
        let receive = pipeline
            .by_name("receive")
            .and_then(|element| element.static_pad("sink"))
            .ok_or("Missing audio receiver.")?;
        peer.connect_pad_added(move |_, pad| {
            if pad.direction() == gst::PadDirection::Src && !receive.is_linked() {
                let _ = pad.link(&receive);
            }
        });
        let audio = Self { pipeline, peer };
        microphone.attach(
            audio
                .pipeline
                .by_name("microphone")
                .ok_or("Missing microphone control.")?,
        );
        audio
            .pipeline
            .set_state(gst::State::Playing)
            .map_err(|error| error.to_string())?;
        Ok(audio)
    }

    pub fn accept_answer(&self, answer: &str) -> Result<(), String> {
        let sdp = SDPMessage::parse_buffer(answer.as_bytes()).map_err(|error| error.to_string())?;
        if sdp.medias().count() != 1
            || sdp.media(0).and_then(|media| media.media()) != Some("audio")
        {
            return Err("The WebRTC description must contain one audio stream.".into());
        }
        self.set_description(
            "set-remote-description",
            &WebRTCSessionDescription::new(WebRTCSDPType::Answer, sdp),
        )
    }

    pub fn gather_offer(&self, cancelled: &AtomicBool) -> Result<String, String> {
        let offer = self.wait_for("create-offer", &[&None::<gst::Structure>], |reply| {
            reply
                .ok_or("WebRTC did not return a session description.".to_owned())?
                .get::<WebRTCSessionDescription>("offer")
                .map_err(|error| error.to_string())
        })?;
        self.set_description("set-local-description", &offer)?;
        self.gather_description(cancelled)
    }

    fn gather_description(&self, cancelled: &AtomicBool) -> Result<String, String> {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline && !cancelled.load(Ordering::Relaxed) {
            self.check_error()?;
            if self
                .peer
                .property::<WebRTCICEGatheringState>("ice-gathering-state")
                == WebRTCICEGatheringState::Complete
            {
                return self
                    .peer
                    .property::<Option<WebRTCSessionDescription>>("local-description")
                    .ok_or("Missing local audio offer.".to_owned())?
                    .sdp()
                    .as_text()
                    .map_err(|error| error.to_string());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        Err("Could not gather audio connection candidates.".into())
    }

    pub fn connection_state(&self) -> WebRTCPeerConnectionState {
        self.peer.property("connection-state")
    }

    fn set_description(
        &self,
        operation: &str,
        description: &WebRTCSessionDescription,
    ) -> Result<(), String> {
        self.wait_for(operation, &[description], |_| Ok(()))
    }

    fn wait_for<T: Send + 'static>(
        &self,
        operation: &str,
        arguments: &[&dyn gst::glib::value::ToValue],
        parse: impl FnOnce(Option<&gst::StructureRef>) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let (send, receive) = mpsc::sync_channel(1);
        let promise = gst::Promise::with_change_func(move |reply| {
            let result = reply
                .map_err(|error| format!("WebRTC operation failed: {error:?}"))
                .and_then(|reply| {
                    if let Some(error) =
                        reply.and_then(|reply| reply.get::<gst::glib::Error>("error").ok())
                    {
                        return Err(error.to_string());
                    }
                    parse(reply)
                });
            let _ = send.send(result);
        });
        let mut arguments = arguments.to_vec();
        arguments.push(&promise);
        self.peer.emit_by_name::<()>(operation, &arguments);
        receive
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| format!("{operation}: {error}"))?
    }

    pub fn check_error(&self) -> Result<(), String> {
        let bus = self.pipeline.bus().ok_or("Missing audio pipeline bus.")?;
        if let Some(message) = bus.pop_filtered(&[gst::MessageType::Error]) {
            if let gst::MessageView::Error(error) = message.view() {
                return Err(error.error().to_string());
            }
        }
        Ok(())
    }
}

impl Drop for AudioPeer {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
