//! The views and the widgets they're built from.

#[cfg(feature = "call-audio")]
pub(crate) mod call;
mod composer;
pub(crate) mod insights;
mod mentions;
mod main_view;
mod media;
mod message;
mod scroll;
mod search;
mod time;
mod widgets;

pub(crate) use {composer::*, mentions::*, main_view::*, media::*, message::*, scroll::*, time::*, widgets::*};
