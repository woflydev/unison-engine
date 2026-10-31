//! Server folder layout: the pure edits (move a server, make a folder, rename, recolor, reorder) and
//! their protobuf encoding for the settings PATCH. The layout is the ordered list of top-level rail
//! entries; a server outside any folder is a one-server entry with no id, as Discord stores it.

use crate::model::{GuildFolder, Id};

/// Something at the top level of the rail, used to say "put it before this".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Guild(Id),
    Folder(i64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// At the top level, before `before` (the end when `None`).
    Top { before: Option<Anchor> },
    /// Inside a folder, before one of its servers (the end when `None`).
    InFolder { id: i64, before: Option<Id> },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    MoveGuild { guild: Id, to: Place },
    /// Drop `guild` onto `onto`: both end up in a new folder (or, if `onto` is in a folder, `guild` joins it).
    MakeFolder { guild: Id, onto: Id },
    MoveFolder { id: i64, before: Option<Anchor> },
    Rename { id: i64, name: String },
    Recolor { id: i64, color: Option<u64> },
}

fn single(g: Id) -> GuildFolder {
    GuildFolder { id: None, name: None, color: None, guild_ids: vec![g] }
}

/// Take `g` out of wherever it is; folders that become empty disappear.
fn remove_guild(layout: &mut Vec<GuildFolder>, g: Id) -> bool {
    let mut found = false;
    for f in layout.iter_mut() {
        let before = f.guild_ids.len();
        f.guild_ids.retain(|x| *x != g);
        found |= f.guild_ids.len() != before;
    }
    layout.retain(|f| !f.guild_ids.is_empty());
    found
}

fn anchor_index(layout: &[GuildFolder], a: Anchor) -> Option<usize> {
    layout.iter().position(|f| match a {
        Anchor::Folder(id) => f.id == Some(id),
        Anchor::Guild(g) => f.id.is_none() && f.guild_ids.first() == Some(&g),
    })
}

fn insert_top(layout: &mut Vec<GuildFolder>, entry: GuildFolder, before: Option<Anchor>) -> Option<()> {
    let at = match before {
        Some(a) => anchor_index(layout, a)?,
        None => layout.len(),
    };
    layout.insert(at, entry);
    Some(())
}

/// Apply an edit to a layout. `new_id` names a folder this edit creates. `None` when the edit
/// refers to something that isn't there (the caller leaves the layout alone).
pub fn apply(layout: &[GuildFolder], edit: &Edit, new_id: i64) -> Option<Vec<GuildFolder>> {
    let mut l = layout.to_vec();
    match edit {
        Edit::MoveGuild { guild, to } => {
            if !l.iter().any(|f| f.guild_ids.contains(guild)) {
                return None;
            }
            // Resolve the destination first: removing the server may drop an empty folder or the anchor's neighbour.
            match *to {
                Place::Top { before } => {
                    if before == Some(Anchor::Guild(*guild)) {
                        return Some(l);
                    }
                    remove_guild(&mut l, *guild);
                    insert_top(&mut l, single(*guild), before)?;
                }
                Place::InFolder { id, before } => {
                    if !l.iter().any(|f| f.id == Some(id)) {
                        return None;
                    }
                    remove_guild(&mut l, *guild);
                    let f = l.iter_mut().find(|f| f.id == Some(id))?;
                    let at = match before {
                        Some(b) => f.guild_ids.iter().position(|x| *x == b).unwrap_or(f.guild_ids.len()),
                        None => f.guild_ids.len(),
                    };
                    f.guild_ids.insert(at, *guild);
                }
            }
        }
        Edit::MakeFolder { guild, onto } => {
            if !l.iter().any(|f| f.guild_ids.contains(guild)) {
                return None;
            }
            // Dropped on itself (the drag preview sits right over it): nothing to do.
            if guild == onto {
                return Some(l);
            }
            let target = l.iter().find(|f| f.guild_ids.contains(onto))?.clone();
            remove_guild(&mut l, *guild);
            let f = l.iter_mut().find(|f| f.guild_ids.contains(onto))?;
            if f.id.is_some() {
                let at = f.guild_ids.iter().position(|x| x == onto).map_or(f.guild_ids.len(), |i| i + 1);
                f.guild_ids.insert(at, *guild);
            } else {
                *f = GuildFolder { id: Some(new_id), name: None, color: None, guild_ids: vec![*onto, *guild] };
            }
            let _ = target;
        }
        Edit::MoveFolder { id, before } => {
            if *before == Some(Anchor::Folder(*id)) {
                return Some(l);
            }
            let i = l.iter().position(|f| f.id == Some(*id))?;
            let f = l.remove(i);
            insert_top(&mut l, f, *before)?;
        }
        Edit::Rename { id, name } => {
            let f = l.iter_mut().find(|f| f.id == Some(*id))?;
            let n = name.trim();
            f.name = (!n.is_empty()).then(|| n.to_string());
        }
        Edit::Recolor { id, color } => {
            let f = l.iter_mut().find(|f| f.id == Some(*id))?;
            f.color = *color;
        }
    }
    Some(l)
}

/// The colours the folder editor offers (name, 0xRRGGBB), as the official client's palette.
/// `capture`: values approximate the web client's folder colours.
pub const PALETTE: [(&str, u32); 8] = [
    ("Blue", 0x5865f2),
    ("Green", 0x3ba55d),
    ("Yellow", 0xfaa81a),
    ("Orange", 0xe67e22),
    ("Red", 0xed4245),
    ("Pink", 0xeb459e),
    ("Purple", 0x9b59b6),
    ("Gray", 0x747f8d),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snowflake::Snowflake;

    fn g(n: u64) -> Id {
        Snowflake(n)
    }
    fn layout() -> Vec<GuildFolder> {
        vec![
            single(g(1)),
            GuildFolder { id: Some(7), name: Some("Work".into()), color: None, guild_ids: vec![g(2), g(3)] },
            single(g(4)),
        ]
    }
    fn ids(l: &[GuildFolder]) -> Vec<Vec<u64>> {
        l.iter().map(|f| f.guild_ids.iter().map(|x| x.0).collect()).collect()
    }

    #[test]
    fn move_to_top_level_position() {
        let l = apply(&layout(), &Edit::MoveGuild { guild: g(4), to: Place::Top { before: Some(Anchor::Guild(g(1))) } }, 0).unwrap();
        assert_eq!(ids(&l), [vec![4], vec![1], vec![2, 3]]);
        let l = apply(&layout(), &Edit::MoveGuild { guild: g(1), to: Place::Top { before: None } }, 0).unwrap();
        assert_eq!(ids(&l), [vec![2, 3], vec![4], vec![1]]);
    }

    #[test]
    fn move_into_and_out_of_a_folder() {
        let l = apply(&layout(), &Edit::MoveGuild { guild: g(4), to: Place::InFolder { id: 7, before: Some(g(3)) } }, 0).unwrap();
        assert_eq!(ids(&l), [vec![1], vec![2, 4, 3]]);
        let out = apply(&l, &Edit::MoveGuild { guild: g(4), to: Place::Top { before: None } }, 0).unwrap();
        assert_eq!(ids(&out), [vec![1], vec![2, 3], vec![4]]);
    }

    #[test]
    fn emptied_folders_disappear() {
        let l = apply(&layout(), &Edit::MoveGuild { guild: g(2), to: Place::Top { before: None } }, 0).unwrap();
        let l = apply(&l, &Edit::MoveGuild { guild: g(3), to: Place::Top { before: None } }, 0).unwrap();
        assert!(l.iter().all(|f| f.id.is_none()));
        assert_eq!(l.len(), 4);
    }

    #[test]
    fn dropping_onto_a_server_makes_a_folder() {
        let l = apply(&layout(), &Edit::MakeFolder { guild: g(4), onto: g(1) }, 99).unwrap();
        assert_eq!(ids(&l), [vec![1, 4], vec![2, 3]]);
        assert_eq!(l[0].id, Some(99));
        // Onto a server already in a folder: it joins that folder.
        let l = apply(&layout(), &Edit::MakeFolder { guild: g(4), onto: g(2) }, 99).unwrap();
        assert_eq!(ids(&l), [vec![1], vec![2, 4, 3]]);
        assert_eq!(l[1].id, Some(7));
        // Onto itself changes nothing and is not an error.
        assert_eq!(apply(&layout(), &Edit::MakeFolder { guild: g(1), onto: g(1) }, 99).unwrap(), layout());
    }

    #[test]
    fn a_single_server_can_be_moved_around_without_error() {
        let one = vec![single(g(1))];
        for to in [Place::Top { before: None }, Place::Top { before: Some(Anchor::Guild(g(1))) }] {
            assert_eq!(apply(&one, &Edit::MoveGuild { guild: g(1), to }, 0).unwrap(), one);
        }
        assert_eq!(apply(&one, &Edit::MakeFolder { guild: g(1), onto: g(1) }, 0).unwrap(), one);
        assert!(apply(&one, &Edit::MoveGuild { guild: g(9), to: Place::Top { before: None } }, 0).is_none());
    }

    #[test]
    fn reorder_rename_and_recolor() {
        let l = apply(&layout(), &Edit::MoveFolder { id: 7, before: None }, 0).unwrap();
        assert_eq!(ids(&l), [vec![1], vec![4], vec![2, 3]]);
        let l = apply(&l, &Edit::Rename { id: 7, name: "  Play ".into() }, 0).unwrap();
        assert_eq!(l[2].name.as_deref(), Some("Play"));
        let l = apply(&l, &Edit::Rename { id: 7, name: " ".into() }, 0).unwrap();
        assert_eq!(l[2].name, None);
        let l = apply(&l, &Edit::Recolor { id: 7, color: Some(0xff0000) }, 0).unwrap();
        assert_eq!(l[2].color, Some(0xff0000));
        assert!(apply(&l, &Edit::Rename { id: 8, name: "x".into() }, 0).is_none());
    }

    #[test]
    fn encoding_round_trips_through_the_reader() {
        let l = apply(&layout(), &Edit::Recolor { id: 7, color: Some(0x123456) }, 0).unwrap();
        let bytes = crate::model::settings_proto::encode_folders(&l);
        let (back, _) = crate::model::settings_proto::guild_folders(&bytes).unwrap();
        assert_eq!(back, l);
    }
}
