//! Upstream `StaticHandler::try_serve`'s filesystem-path policy, stepped between the
//! caller's effects.
use super::{HttpStaticContext, INVALID, StaticText::Filename};
use crate::static_meta::static_catch;
use crate::static_path::{self as path, Action};

#[derive(Default)]
pub(super) struct Serve {
    facts: u32,
    /// The action the last step asked for, which the next event answers.
    pending: Option<Action>,
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
    /// Resume the path policy with the last effect's observation: the next action,
    /// never [`Action::Directory`] or [`Action::Catch`], which complete here.
    pub(super) fn serve_step(&mut self, root: &[u8], event: &Event<'_>) -> Result<Action, i32> {
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
            Event::Locations if self.serve.pending == Some(Action::Locations) => {}
            Event::Stat(observed) if self.serve.pending == Some(Action::Lstat) => {
                self.serve.facts &= !(path::STAT_OK | path::LINK | path::DIRECTORY | path::FILE);
                if observed {
                    self.serve.facts |= path::STAT_OK | self.facts.kind;
                }
            }
            Event::Canonical(value) if self.serve.pending == Some(Action::Realpath) => {
                self.serve.facts &= !path::CANONICAL_OK;
                if let Some(value) = value
                    && value.len() < self.capacity
                {
                    let status = self.publish(Filename, value);
                    if status < 0 {
                        return Err(status);
                    }
                    self.serve.facts |= path::CANONICAL_OK;
                }
            }
            Event::Mime(observed) if self.serve.pending == Some(Action::Mime) => {
                self.serve.facts &= !path::MIME;
                if observed {
                    self.serve.facts |= path::MIME;
                }
            }
            _ => return Err(INVALID),
        }
        let action = loop {
            match self.path_step(root, self.serve.facts) {
                // Directory bytes were published by the owned path step.
                Ok(Action::Directory) => {}
                Ok(Action::Catch) => {
                    let found = static_catch(self.path.last, &mut self.facts.status);
                    break Ok(Action::Done(found));
                }
                action => break action,
            }
        };
        self.serve.pending = action.ok();
        action
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
            assert_eq!(owner.serve_step(ROOT, &begin(true)), Ok(Action::Locations));
            assert_eq!(owner.location(b"/"), Ok(true));
            assert_eq!(owner.serve_step(ROOT, &Event::Locations), Ok(Action::Lstat));
            stat(&mut owner, path::FILE);
            assert_eq!(owner.serve_step(ROOT, &Event::Stat(true)), Ok(Action::Mime));
            assert_eq!(
                owner.serve_step(ROOT, &Event::Mime(true)),
                Ok(Action::Done(true))
            );
            assert_eq!(owner.path.last, 1);
        }
    }
    #[test]
    fn canonical_observation_admits_owned_filename_before_following_stat() {
        let mut owner = HttpStaticContext::new(b"/alias.txt", 4096).unwrap();
        assert_eq!(owner.serve_step(ROOT, &begin(false)), Ok(Action::Locations));
        assert_eq!(owner.serve_step(ROOT, &Event::Locations), Ok(Action::Lstat));
        stat(&mut owner, path::LINK);
        assert_eq!(
            owner.serve_step(ROOT, &Event::Stat(true)),
            Ok(Action::Realpath)
        );
        assert_eq!(
            owner.serve_step(ROOT, &Event::Canonical(Some(b"/root/file.txt"))),
            Ok(Action::Lstat)
        );
        assert_eq!(&owner.filename[..15], b"/root/file.txt\0");
        stat(&mut owner, path::FILE);
        assert_eq!(owner.serve_step(ROOT, &Event::Stat(true)), Ok(Action::Mime));
        assert_eq!(
            owner.serve_step(ROOT, &Event::Mime(true)),
            Ok(Action::Done(true))
        );
        assert_eq!(owner.serve_step(ROOT, &Event::Mime(true)), Err(INVALID));
    }
}
