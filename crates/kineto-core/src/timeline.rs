//! The production package's timeline: the locked shots end to end on a
//! video track, and each shot's locked lines on a dialogue track where the
//! shot holds them, written for an editor to open as a rough cut.
//!
//! Times are whole frames at [`FRAME_RATE`]. A shot lasts as long as its
//! file does, read from the MP4's `mvhd` box without decoding; the length
//! the take was asked for is used only when the file does not say. A shot's
//! lines follow one another from its start with [`BREATH_MS`] between them,
//! as planning times them, and a line running past its shot's end is cut
//! there and reported.
//!
//! The same placement is written as OpenTimelineIO ([`otio`]) and as a CMX
//! 3600 edit decision list ([`edl`]), so the two never disagree.

use serde_json::{Value, json};

/// Frames a second of every time on the timeline.
pub const FRAME_RATE: u32 = 24;

/// The breath between two lines one shot carries, as planning times it.
pub const BREATH_MS: u32 = 250;

/// One locked line a shot carries, as the package holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineLine {
    /// Its place among its scene's lines, from 1.
    pub line: u32,
    pub character: String,
    /// The audio file, relative to the package.
    pub file: String,
    pub artifact_id: String,
    pub content_hash: String,
    pub duration_ms: u32,
}

/// One locked shot, in the order the package plays it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineShot {
    pub scene_id: String,
    pub shot_id: String,
    /// The video file, relative to the package.
    pub file: String,
    pub artifact_id: String,
    pub content_hash: String,
    pub duration_ms: u32,
    pub lines: Vec<TimelineLine>,
}

/// A line cut at the end of the shot that carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trimmed {
    pub shot_id: String,
    pub line: u32,
    /// How much of it did not fit, in milliseconds.
    pub cut_ms: u32,
}

/// Where everything sits, in frames from the start of the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed<'a> {
    pub shots: Vec<(&'a TimelineShot, u64, u64)>,
    /// Each line with its start and length, in timeline order.
    pub lines: Vec<(&'a TimelineShot, &'a TimelineLine, u64, u64)>,
    pub trimmed: Vec<Trimmed>,
}

/// `ms` in whole frames, to the nearest.
#[must_use]
pub fn frames(ms: u64) -> u64 {
    (ms * u64::from(FRAME_RATE) + 500) / 1000
}

/// Lay `shots` end to end, and each one's lines within it.
#[must_use]
pub fn place(shots: &[TimelineShot]) -> Placed<'_> {
    let mut placed = Placed {
        shots: Vec::with_capacity(shots.len()),
        lines: Vec::new(),
        trimmed: Vec::new(),
    };
    let mut start = 0;
    for shot in shots {
        let length = frames(u64::from(shot.duration_ms));
        placed.shots.push((shot, start, length));
        let end_ms = u64::from(shot.duration_ms);
        let mut offset_ms: u64 = 0;
        for (index, line) in shot.lines.iter().enumerate() {
            if index > 0 {
                offset_ms += u64::from(BREATH_MS);
            }
            let said_ms = u64::from(line.duration_ms);
            let kept_ms = said_ms.min(end_ms.saturating_sub(offset_ms));
            if kept_ms < said_ms {
                placed.trimmed.push(Trimmed {
                    shot_id: shot.shot_id.clone(),
                    line: line.line,
                    cut_ms: u32::try_from(said_ms - kept_ms).unwrap_or(u32::MAX),
                });
            }
            let line_start = (start + frames(offset_ms)).min(start + length);
            let line_length = frames(offset_ms + kept_ms)
                .min(length)
                .saturating_sub(frames(offset_ms).min(length));
            if line_length > 0 {
                placed.lines.push((shot, line, line_start, line_length));
            }
            offset_ms += said_ms;
        }
        start += length;
    }
    placed
}

fn rational(frames: u64) -> Value {
    json!({
        "OTIO_SCHEMA": "RationalTime.1",
        "rate": f64::from(FRAME_RATE),
        "value": frames as f64,
    })
}

fn range(start: u64, length: u64) -> Value {
    json!({
        "OTIO_SCHEMA": "TimeRange.1",
        "start_time": rational(start),
        "duration": rational(length),
    })
}

fn clip(name: &str, file: &str, length: u64, metadata: Value) -> Value {
    json!({
        "OTIO_SCHEMA": "Clip.1",
        "name": name,
        "metadata": { "kineto": metadata },
        "source_range": range(0, length),
        "effects": [],
        "markers": [],
        "enabled": true,
        "media_reference": {
            "OTIO_SCHEMA": "ExternalReference.1",
            "name": file,
            "metadata": {},
            "available_range": range(0, length),
            "target_url": file,
        },
    })
}

fn gap(length: u64) -> Value {
    json!({
        "OTIO_SCHEMA": "Gap.1",
        "name": "",
        "metadata": {},
        "source_range": range(0, length),
        "effects": [],
        "markers": [],
        "enabled": true,
    })
}

fn track(name: &str, kind: &str, children: Vec<Value>) -> Value {
    json!({
        "OTIO_SCHEMA": "Track.1",
        "name": name,
        "kind": kind,
        "metadata": {},
        "source_range": null,
        "effects": [],
        "markers": [],
        "enabled": true,
        "children": children,
    })
}

/// The OpenTimelineIO timeline of `placed`, named `title`: a video track of
/// the shots and a dialogue track of their lines, with gaps between, every
/// clip naming its file relative to the package and carrying its Kineto
/// identity.
#[must_use]
pub fn otio(title: &str, placed: &Placed<'_>) -> Value {
    let video = placed
        .shots
        .iter()
        .map(|(shot, _, length)| {
            clip(
                &shot.shot_id,
                &shot.file,
                *length,
                json!({
                    "scene_id": shot.scene_id,
                    "shot_id": shot.shot_id,
                    "artifact_id": shot.artifact_id,
                    "content_hash": shot.content_hash,
                }),
            )
        })
        .collect();
    let mut dialogue = Vec::new();
    let mut at = 0;
    for (shot, line, start, length) in &placed.lines {
        if *start > at {
            dialogue.push(gap(start - at));
        }
        dialogue.push(clip(
            &format!("{} line {}", shot.shot_id, line.line),
            &line.file,
            *length,
            json!({
                "scene_id": shot.scene_id,
                "shot_id": shot.shot_id,
                "line": line.line,
                "character": line.character,
                "artifact_id": line.artifact_id,
                "content_hash": line.content_hash,
            }),
        ));
        at = start + length;
    }
    json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": title,
        "metadata": { "kineto": { "frame_rate": FRAME_RATE } },
        "global_start_time": rational(0),
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "name": "tracks",
            "metadata": {},
            "source_range": null,
            "effects": [],
            "markers": [],
            "enabled": true,
            "children": [
                track("Video", "Video", video),
                track("Dialogue", "Audio", dialogue),
            ],
        },
    })
}

/// The most events a CMX 3600 list numbers.
pub const MAX_EDL_EVENTS: usize = 999;

/// The longest title a CMX 3600 list carries; a longer one is cut.
const EDL_TITLE_CHARS: usize = 70;

/// `frames` as non-drop timecode at [`FRAME_RATE`], `HH:MM:SS:FF`.
#[must_use]
pub fn timecode(frames: u64) -> String {
    let rate = u64::from(FRAME_RATE);
    let seconds = frames / rate;
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        frames % rate
    )
}

/// The CMX 3600 edit decision list of `placed`, titled `title`: one video
/// event a shot and one audio event a line, in record order, at non-drop
/// [`FRAME_RATE`]. Each event's source runs from the start of its file, and
/// a comment names the file relative to the package. Silence before the
/// first line is a blank (`BL`) event, so a reader does not start the sound
/// at the first line. Refused when there would be more than
/// [`MAX_EDL_EVENTS`] events.
pub fn edl(title: &str, placed: &Placed<'_>) -> Result<String, String> {
    let mut events: Vec<(u64, &str, Option<&str>, u64)> = placed
        .shots
        .iter()
        .map(|(shot, start, length)| (*start, "V", Some(shot.file.as_str()), *length))
        .chain(
            placed
                .lines
                .iter()
                .map(|(_, line, start, length)| (*start, "A", Some(line.file.as_str()), *length)),
        )
        .collect();
    if let Some((_, _, first, _)) = placed.lines.first()
        && *first > 0
    {
        events.push((0, "A", None, *first));
    }
    if events.len() > MAX_EDL_EVENTS {
        return Err(format!(
            "an edit decision list holds at most {MAX_EDL_EVENTS} events; this one would hold {}",
            events.len()
        ));
    }
    // Record order, the picture before the sound that starts with it.
    events.sort_by_key(|(start, track, _, _)| (*start, *track == "A"));
    let title: String = title
        .chars()
        .filter(|character| !character.is_control())
        .take(EDL_TITLE_CHARS)
        .collect();
    let mut list = format!("TITLE: {title}\nFCM: NON-DROP FRAME\n");
    for (number, (start, track, file, length)) in (1..).zip(events) {
        list.push_str(&format!(
            "\n{number:03}  {:<8} {track:<5} C        {} {} {} {}\n",
            if file.is_some() { "AX" } else { "BL" },
            timecode(0),
            timecode(length),
            timecode(start),
            timecode(start + length),
        ));
        if let Some(file) = file {
            list.push_str(&format!("* FROM CLIP NAME: {file}\n"));
        }
    }
    Ok(list)
}

/// How long the MP4 `bytes` lasts, in milliseconds, from its movie header
/// (`moov`/`mvhd`); `None` when it has none or it does not say.
#[must_use]
pub fn mp4_duration_ms(bytes: &[u8]) -> Option<u32> {
    let moov = child(bytes, *b"moov")?;
    let mvhd = child(moov, *b"mvhd")?;
    let (version, rest) = mvhd.split_first()?;
    let rest = rest.get(3..)?;
    let (timescale, duration) = match version {
        0 => (
            u32::from_be_bytes(rest.get(8..12)?.try_into().ok()?),
            u64::from(u32::from_be_bytes(rest.get(12..16)?.try_into().ok()?)),
        ),
        1 => (
            u32::from_be_bytes(rest.get(16..20)?.try_into().ok()?),
            u64::from_be_bytes(rest.get(20..28)?.try_into().ok()?),
        ),
        _ => return None,
    };
    if timescale == 0 || duration == u64::from(u32::MAX) || duration == u64::MAX {
        return None;
    }
    u32::try_from(duration * 1000 / u64::from(timescale)).ok()
}

/// The content of the first box of `kind` among the boxes in `bytes`.
fn child(mut bytes: &[u8], kind: [u8; 4]) -> Option<&[u8]> {
    while bytes.len() >= 8 {
        let size = u64::from(u32::from_be_bytes(bytes[0..4].try_into().ok()?));
        let (header, size) = match size {
            0 => (8, bytes.len() as u64),
            1 => (16, u64::from_be_bytes(bytes.get(8..16)?.try_into().ok()?)),
            size => (8, size),
        };
        let size = usize::try_from(size).ok()?;
        if size < header || size > bytes.len() {
            return None;
        }
        if bytes[4..8] == kind {
            return Some(&bytes[header..size]);
        }
        bytes = &bytes[size..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(kind: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let mut bytes = u32::try_from(content.len() + 8)
            .unwrap()
            .to_be_bytes()
            .to_vec();
        bytes.extend_from_slice(kind);
        bytes.extend_from_slice(content);
        bytes
    }

    fn mvhd_v0(timescale: u32, duration: u32) -> Vec<u8> {
        let mut content = vec![0, 0, 0, 0];
        content.extend_from_slice(&[0; 8]);
        content.extend_from_slice(&timescale.to_be_bytes());
        content.extend_from_slice(&duration.to_be_bytes());
        content.extend_from_slice(&[0; 80]);
        boxed(b"mvhd", &content)
    }

    fn mvhd_v1(timescale: u32, duration: u64) -> Vec<u8> {
        let mut content = vec![1, 0, 0, 0];
        content.extend_from_slice(&[0; 16]);
        content.extend_from_slice(&timescale.to_be_bytes());
        content.extend_from_slice(&duration.to_be_bytes());
        content.extend_from_slice(&[0; 80]);
        boxed(b"mvhd", &content)
    }

    #[test]
    fn a_files_length_is_read_from_its_movie_header() {
        let ftyp = boxed(b"ftyp", b"isom\0\0\x02\0isomiso2");
        let mdat = boxed(b"mdat", &[7; 64]);
        let mut file = ftyp.clone();
        file.extend(mdat.clone());
        file.extend(boxed(b"moov", &mvhd_v0(12_800, 75_264)));
        assert_eq!(mp4_duration_ms(&file), Some(5_880));

        // A 64-bit header, and a movie box ahead of the media.
        let mut file = ftyp.clone();
        file.extend(boxed(b"moov", &mvhd_v1(1_000, 10_130)));
        file.extend(mdat);
        assert_eq!(mp4_duration_ms(&file), Some(10_130));

        // No movie box, no timescale, or a box claiming more than there is.
        assert_eq!(mp4_duration_ms(&ftyp), None);
        let mut file = ftyp.clone();
        file.extend(boxed(b"moov", &mvhd_v0(0, 100)));
        assert_eq!(mp4_duration_ms(&file), None);
        let mut broken = boxed(b"moov", &mvhd_v0(1_000, 100));
        broken[3] = 0xff;
        assert_eq!(mp4_duration_ms(&broken), None);

        // The bundled demo clip lasts two seconds.
        let demo = include_bytes!("../assets/demo-clip.mp4");
        assert_eq!(mp4_duration_ms(demo).map(|ms| ms / 100), Some(20));
    }

    fn line(line: u32, duration_ms: u32) -> TimelineLine {
        TimelineLine {
            line,
            character: "owen".to_owned(),
            file: format!("shot_line{line:02}.mp3"),
            artifact_id: format!("line.scene.{line:02}.0001"),
            content_hash: format!("sha256:{}", "a".repeat(64)),
            duration_ms,
        }
    }

    fn shot(id: &str, duration_ms: u32, lines: Vec<TimelineLine>) -> TimelineShot {
        TimelineShot {
            scene_id: "scene".to_owned(),
            shot_id: id.to_owned(),
            file: format!("{id}.mp4"),
            artifact_id: format!("{id}_v0001_candidate_a"),
            content_hash: format!("sha256:{}", "b".repeat(64)),
            duration_ms,
            lines,
        }
    }

    #[test]
    fn shots_play_end_to_end_and_lines_sit_in_their_shot() {
        let shots = [
            shot("scene_01", 5_880, Vec::new()),
            shot("scene_02", 6_000, vec![line(1, 2_000), line(2, 3_000)]),
            // A line longer than its shot is cut at the shot's end.
            shot("scene_03", 2_000, vec![line(3, 2_900)]),
        ];
        let placed = place(&shots);
        let spans: Vec<_> = placed
            .shots
            .iter()
            .map(|(shot, start, length)| (shot.shot_id.as_str(), *start, *length))
            .collect();
        assert_eq!(
            spans,
            [
                ("scene_01", 0, 141),
                ("scene_02", 141, 144),
                ("scene_03", 285, 48)
            ]
        );
        let lines: Vec<_> = placed
            .lines
            .iter()
            .map(|(_, line, start, length)| (line.line, *start, *length))
            .collect();
        // Line 2 starts after line 1 and a breath: 2.25 s into the shot.
        assert_eq!(lines, [(1, 141, 48), (2, 195, 72), (3, 285, 48)]);
        assert_eq!(
            placed.trimmed,
            [Trimmed {
                shot_id: "scene_03".to_owned(),
                line: 3,
                cut_ms: 900,
            }]
        );
    }

    #[test]
    fn the_edit_decision_list_is_the_same_timeline_in_cmx_3600() {
        assert_eq!(timecode(0), "00:00:00:00");
        assert_eq!(timecode(23), "00:00:00:23");
        assert_eq!(timecode(24 * 3_661 + 5), "01:01:01:05");
        let shots = [
            shot("scene_01", 2_000, Vec::new()),
            shot("scene_02", 4_000, vec![line(1, 1_000)]),
        ];
        let list = edl("The Letters", &place(&shots)).unwrap();
        assert_eq!(
            list,
            "TITLE: The Letters\nFCM: NON-DROP FRAME\n\
             \n001  AX       V     C        00:00:00:00 00:00:02:00 00:00:00:00 00:00:02:00\n\
             * FROM CLIP NAME: scene_01.mp4\n\
             \n002  BL       A     C        00:00:00:00 00:00:02:00 00:00:00:00 00:00:02:00\n\
             \n003  AX       V     C        00:00:00:00 00:00:04:00 00:00:02:00 00:00:06:00\n\
             * FROM CLIP NAME: scene_02.mp4\n\
             \n004  AX       A     C        00:00:00:00 00:00:01:00 00:00:02:00 00:00:03:00\n\
             * FROM CLIP NAME: shot_line01.mp3\n"
        );
        // Too many events is refused, not cut short.
        let many: Vec<_> = (0..1_000)
            .map(|index| shot(&format!("s{index}"), 1_000, Vec::new()))
            .collect();
        assert!(
            edl("Many", &place(&many))
                .unwrap_err()
                .contains("at most 999 events")
        );
        // A long title is cut to what the format carries.
        let long = edl(&"T".repeat(200), &place(&shots)).unwrap();
        assert_eq!(long.lines().next().unwrap().len(), "TITLE: ".len() + 70);
    }

    #[test]
    fn the_timeline_is_opentimelineio_with_gaps_between_lines() {
        let shots = [
            shot("scene_01", 2_000, Vec::new()),
            shot("scene_02", 4_000, vec![line(1, 1_000)]),
        ];
        let timeline = otio("The Letters", &place(&shots));
        assert_eq!(timeline["OTIO_SCHEMA"], "Timeline.1");
        assert_eq!(timeline["name"], "The Letters");
        let tracks = timeline["tracks"]["children"].as_array().unwrap();
        assert_eq!(
            (tracks[0]["kind"].as_str(), tracks[1]["kind"].as_str()),
            (Some("Video"), Some("Audio"))
        );
        let video = tracks[0]["children"].as_array().unwrap();
        assert_eq!(video[1]["media_reference"]["target_url"], "scene_02.mp4");
        assert_eq!(video[1]["source_range"]["duration"]["value"], 96.0);
        assert_eq!(
            video[1]["metadata"]["kineto"]["artifact_id"],
            "scene_02_v0001_candidate_a"
        );
        let dialogue = tracks[1]["children"].as_array().unwrap();
        let kinds: Vec<_> = dialogue
            .iter()
            .map(|item| item["OTIO_SCHEMA"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["Gap.1", "Clip.1"]);
        assert_eq!(dialogue[0]["source_range"]["duration"]["value"], 48.0);
        assert_eq!(dialogue[1]["metadata"]["kineto"]["line"], 1);
        assert_eq!(dialogue[1]["source_range"]["duration"]["rate"], 24.0);
    }
}
