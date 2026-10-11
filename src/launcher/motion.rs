use super::app::Page;
use std::time::{Duration, Instant};

const DURATION: Duration = Duration::from_millis(170);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Menu {
    Profile,
    StartRule,
    Speed,
    Slot(u8),
}

impl Menu {
    pub fn index(self) -> usize {
        match self {
            Self::Profile => 0,
            Self::StartRule => 1,
            Self::Speed => 2,
            Self::Slot(slot) => 3 + usize::from(slot.min(23)),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Tween {
    from: f32,
    to: f32,
    at: Instant,
}

impl Tween {
    pub fn settled(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            at: Instant::now(),
        }
    }

    pub fn value(&self, now: Instant) -> f32 {
        let time = (now.saturating_duration_since(self.at).as_secs_f32() / DURATION.as_secs_f32())
            .min(1.0);
        let eased = 1.0 - (1.0 - time).powi(3);
        self.from + (self.to - self.from) * eased
    }

    pub fn target(&mut self, to: f32, now: Instant) {
        if self.to != to {
            self.from = self.value(now);
            self.to = to;
            self.at = now;
        }
    }

    pub fn active(&self, now: Instant) -> bool {
        self.from != self.to && now.saturating_duration_since(self.at) < DURATION
    }

    pub fn is_open(&self) -> bool {
        self.to == 1.0
    }
}

pub struct Motion {
    pub now: Instant,
    pub page: Tween,
    pub pending_page: Option<Page>,
    pub modal: Tween,
    pub menus: [Tween; 27],
}

impl Default for Motion {
    fn default() -> Self {
        let now = Instant::now();
        let mut page = Tween::settled(0.0);
        page.target(1.0, now);
        Self {
            now,
            page,
            pending_page: None,
            modal: Tween::settled(0.0),
            menus: [Tween::settled(0.0); 27],
        }
    }
}

impl Motion {
    pub fn navigate(&mut self, current: Page, next: Page) {
        self.close_menus();
        if current == next {
            self.pending_page = None;
            self.page.target(1.0, Instant::now());
        } else {
            self.pending_page = Some(next);
            self.page.target(0.0, Instant::now());
        }
    }

    pub fn tick(&mut self, now: Instant) -> Option<Page> {
        self.now = now;
        if self.pending_page.is_some() && !self.page.active(now) {
            let page = self.pending_page.take();
            self.page.target(1.0, now);
            return page;
        }
        None
    }

    pub fn active(&self) -> bool {
        self.pending_page.is_some()
            || self.page.active(self.now)
            || self.modal.active(self.now)
            || self.menus.iter().any(|t| t.active(self.now))
    }

    pub fn close_menus(&mut self) {
        for menu in &mut self.menus {
            menu.target(0.0, Instant::now());
        }
    }

    pub fn toggle_menu(&mut self, menu: Menu) {
        let open = !self.menus[menu.index()].is_open();
        self.close_menus();
        self.menus[menu.index()].target(f32::from(open), Instant::now());
    }

    pub fn modal_visible(&self) -> bool {
        self.modal.is_open() || self.modal.value(self.now) > 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversing_an_animation_is_continuous_and_eventually_sleeps() {
        let now = Instant::now();
        let mut tween = Tween::settled(0.0);
        tween.target(1.0, now);
        let halfway = now + Duration::from_millis(60);
        let before = tween.value(halfway);
        tween.target(0.0, halfway);
        assert_eq!(tween.value(halfway), before);
        assert_eq!(tween.value(halfway + DURATION), 0.0);
        assert!(!tween.active(halfway + DURATION));
    }

    #[test]
    fn fast_navigation_finishes_on_the_latest_page() {
        let mut motion = Motion::default();
        motion.navigate(Page::Hosting, Page::Options);
        motion.navigate(Page::Hosting, Page::Profiles);
        let now = Instant::now() + DURATION;
        assert_eq!(motion.tick(now), Some(Page::Profiles));
        motion.tick(now + DURATION);
        assert!(!motion.active());
    }
}
