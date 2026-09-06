//! A standalone build, used to inspect the editor outside of a host.

use nih_plug::prelude::*;

fn main() {
    nih_export_standalone::<omhemill::Omhemill>();
}
