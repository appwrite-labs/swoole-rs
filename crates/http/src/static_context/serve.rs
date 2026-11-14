//! Upstream `StaticHandler::try_serve`'s filesystem-path policy, stepped between the
//! caller's effects.
use super::{FILENAME, HttpStaticContext, INVALID};
use crate::static_meta::static_catch;
use crate::static_path as path;

#[derive(Default)]
pub(super) struct Serve {
    facts: u32,
    pending: i32,
}
/// What the last effect observed: begin0 (with the configuration), locations-finished1,
/// stat2, canonical3 (with the canonical path) or MIME4.
pub(super) enum Event<'a> {
    Begin {
        has_locations: bool,
        has_index: bool,
        autoindex: bool,
    },
    Locations,
    Stat(bool),
    Canonical(Option<&'a [u8]>),
    Mime(bool),
}
impl HttpStaticContext {
    /// Resume the path policy with the last effect's observation. Returns the path
    /// actions: 0=false, 1=true, 2=visit locations, 4=lstat, 5=realpath, 6=MIME lookup;
    /// directory publication and terminal error handling complete here.
    pub(super) fn serve_step(&mut self, root: &[u8], event: &Event<'_>) -> i32 {
        match *event {
            Event::Begin {
                has_locations,
                has_index,
                autoindex,
            } => {
                self.path.phase = 0;
                self.path.url_length = 0;
                self.serve.facts = if has_locations { path::LOCATIONS } else { 0 }
                    | if has_index { path::INDEX } else { 0 }
                    | if autoindex { path::AUTOINDEX } else { 0 };
            }
            Event::Locations if self.serve.pending == 2 => {}
            Event::Stat(observed) if self.serve.pending == 4 => {
                self.serve.facts &= !(path::STAT_OK | path::LINK | path::DIRECTORY | path::FILE);
                if observed {
                    self.serve.facts |= path::STAT_OK | self.facts.kind;
                }
            }
            Event::Canonical(value) if self.serve.pending == 5 => {
                self.serve.facts &= !path::CANONICAL_OK;
                if let Some(value) = value
                    && value.len() < self.capacity
                {
                    let status = self.publish(FILENAME, value);
                    if status < 0 {
                        return status;
                    }
                    self.serve.facts |= path::CANONICAL_OK;
                }
            }
            Event::Mime(observed) if self.serve.pending == 6 => {
                self.serve.facts &= !path::MIME;
                if observed {
                    self.serve.facts |= path::MIME;
                }
            }
            _ => return INVALID,
        }
        loop {
            let action = self.path_step(root, self.serve.facts);
            match action {
                3 => {} // Directory bytes were published by the owned path step.
                7 => {
                    let result = static_catch(self.path.last, &mut self.facts.status);
                    self.serve.pending = result;
                    return result;
                }
                _ => {
                    self.serve.pending = action;
                    return action;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HttpStaticObservation;
    const ROOT: &[u8] = b"/root";
    fn stat(owner: &mut HttpStaticContext, kind: u32) {
        owner.observed(&HttpStaticObservation {
            filesize: 17,
            kind,
            ..HttpStaticObservation::default()
        });
    }
    fn begin(has_locations: bool) -> Event<'static> {
        Event::Begin {
            has_locations,
            has_index: false,
            autoindex: false,
        }
    }
    #[test]
    fn owned_fact_flow_preserves_configured_location_and_repeated_entry() {
        let mut owner = HttpStaticContext::new(b"/file.txt", 4096).unwrap();
        for _ in 0..2 {
            assert_eq!(owner.serve_step(ROOT, &begin(true)), 2);
            assert_eq!(owner.location(b"/"), Ok(true));
            assert_eq!(owner.serve_step(ROOT, &Event::Locations), 4);
            stat(&mut owner, path::FILE);
            assert_eq!(owner.serve_step(ROOT, &Event::Stat(true)), 6);
            assert_eq!(owner.serve_step(ROOT, &Event::Mime(true)), 1);
            assert_eq!(owner.path.last, 1);
        }
    }
    #[test]
    fn canonical_observation_admits_owned_filename_before_following_stat() {
        let mut owner = HttpStaticContext::new(b"/alias.txt", 4096).unwrap();
        assert_eq!(owner.serve_step(ROOT, &begin(false)), 2);
        assert_eq!(owner.serve_step(ROOT, &Event::Locations), 4);
        stat(&mut owner, path::LINK);
        assert_eq!(owner.serve_step(ROOT, &Event::Stat(true)), 5);
        assert_eq!(
            owner.serve_step(ROOT, &Event::Canonical(Some(b"/root/file.txt"))),
            4
        );
        assert_eq!(&owner.filename[..15], b"/root/file.txt\0");
        stat(&mut owner, path::FILE);
        assert_eq!(owner.serve_step(ROOT, &Event::Stat(true)), 6);
        assert_eq!(owner.serve_step(ROOT, &Event::Mime(true)), 1);
        assert_eq!(owner.serve_step(ROOT, &Event::Mime(true)), INVALID);
    }
}
