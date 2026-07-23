//! The attach/drain flow — the composition proper. Generic over any [`Tap`],
//! so the same flow serves a real cable and a simulated one. Every boundary
//! refuses before use.

use crate::error::Result;
use lucid_sld::enumerate::{enumerate, Enumeration};
use lucid_sld::instrument::{self as lin, Ident, RegionInfo};
use lucid_sld::node::Node;
use lucid_sld::tap::Tap;

/// Everything the read-only commands render from, gathered by one attach.
pub struct Attached {
    /// The hub walk (refused if the hub is not Altera).
    pub enumeration: Enumeration,
    /// The LIN instrument node.
    pub node: Node,
    /// The checked IDENT (refused on magic/proto mismatch).
    pub ident: Ident,
}

/// Attach to a live instrument: enumerate the hub (refuses a non-Altera
/// device, the silicon-witnessed 0x7FF case), attach to the instrument node
/// (refuses a hub with no instrument), and read+check IDENT (refuses a
/// proto/magic mismatch). Each refusal is a typed [`crate::error::HostError`].
pub fn attach<T: Tap>(tap: &mut T) -> Result<Attached> {
    let enumeration = enumerate(tap)?;
    let node = lin::attach(tap)?;
    let ident = lin::ident_checked(tap, &node)?;
    Ok(Attached {
        enumeration,
        node,
        ident,
    })
}

/// A region's descriptor.
pub fn region_info<T: Tap>(tap: &mut T, node: &Node, region: u8) -> Result<RegionInfo> {
    Ok(lin::region_info(tap, node, region)?)
}

/// Drain a region's full contents as 32-bit words.
///
/// Rounds the VDR count UP and truncates, so a region whose `word_count` is not
/// a multiple of `words_per_vdr` still drains every word — a stranger's
/// instrument need not have evenly-sized regions.
pub fn drain_region<T: Tap>(tap: &mut T, node: &Node, region: u8) -> Result<Vec<u32>> {
    let ri = lin::region_info(tap, node, region)?;
    let wpv = u32::from(ri.words_per_vdr.max(1));
    let vdrs = ri.word_count.div_ceil(wpv) as usize;
    let raw = lin::drain(tap, node, region, 0, vdrs)?;
    let mut words = lin::split64(&raw);
    words.truncate(ri.word_count as usize);
    Ok(words)
}
