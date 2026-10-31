//! History windows: a jump opens a page around a message; it becomes the newest page when it
//! reaches the channel's latest message, and grows towards the present one page at a time.

use super::tests::store;
use super::*;

fn plain(channel: u64, id: u64) -> Message {
    Message { id: Snowflake(id), channel_id: Snowflake(channel), ..Default::default() }
}

#[test]
fn window_reaching_the_latest_message_is_the_newest_page() {
    let mut s = store();
    let ch = Snowflake(11); // last_message_id 500
    // Around an old message: not the newest page.
    s.set_history_window(ch, vec![plain(11, 300), plain(11, 290)]);
    assert!(!s.history[&ch].at_latest);
    // Around a recent one: the page includes 500, so a reply jump to it is not "far from present".
    s.set_history_window(ch, vec![plain(11, 500), plain(11, 480), plain(11, 470)]);
    assert!(s.history[&ch].at_latest);
}

#[test]
fn detached_window_grows_to_the_present() {
    let mut s = store();
    let ch = Snowflake(11);
    s.set_history_window(ch, vec![plain(11, 300), plain(11, 290)]);
    // A full page: more to come, still detached.
    s.append_history_newer(ch, vec![plain(11, 400), plain(11, 350), plain(11, 320)], true);
    assert!(!s.history[&ch].at_latest);
    let ids: Vec<u64> = s.history[&ch].messages.iter().map(|m| m.id.0).collect();
    assert_eq!(ids, vec![290, 300, 320, 350, 400]);
    // A short page: the channel's newest message was reached.
    s.append_history_newer(ch, vec![plain(11, 500), plain(11, 450)], false);
    assert!(s.history[&ch].at_latest);
    let ids: Vec<u64> = s.history[&ch].messages.iter().map(|m| m.id.0).collect();
    assert_eq!(ids, vec![290, 300, 320, 350, 400, 450, 500]);
    // Repeats and older ids are ignored.
    s.append_history_newer(ch, vec![plain(11, 500), plain(11, 100)], false);
    assert_eq!(s.history[&ch].messages.len(), 7);
}

#[test]
fn window_tail_is_the_time_gap_to_the_channel_latest() {
    let mut s = store();
    let ch = Snowflake(11);
    let at = |ms: u64| ms << 22;
    s.channels.get_mut(&ch).unwrap().last_message_id = Some(Snowflake(at(10_000_000)));
    s.set_history_window(ch, vec![plain(11, at(4_000_000))]);
    assert_eq!(s.window_tail_ms(ch), 6_000_000);
    // At the newest page there is no tail.
    s.history.get_mut(&ch).unwrap().at_latest = true;
    assert_eq!(s.window_tail_ms(ch), 0);
}
