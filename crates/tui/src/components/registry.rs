use std::collections::HashMap;
use super::*;

struct Slot { generation: u64, component: Option<Box<dyn ErasedComponent>> }
/// The sole built-in composition list. Add a panel here only after deleting its
/// residual enum/reducer/executor/render/print hooks.
pub fn builtin() -> Registry {
    let mut registry = Registry::empty();
    registry.register(update::Update::default()).expect("built-in component registration");
    registry
}

/// Ordered built-in storage, topic/slash indexes and exclusive input focus.
pub struct Registry {
    slots: Vec<Slot>,
    by_topic: HashMap<&'static str, Vec<ComponentToken>>,
    by_slash: HashMap<&'static str, ComponentToken>,
    focus: Option<(ComponentToken, FocusClass)>,
    epoch: u64,
}
impl Default for Registry { fn default() -> Self { Self::empty() } }
impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry").field("slots", &self.slots.len()).field("epoch", &self.epoch).finish_non_exhaustive()
    }
}
impl Registry {
    /// Empty shell for fixed composition and isolated contract consumers.
    pub fn empty() -> Self {
        Self { slots: Vec::new(), by_topic: HashMap::new(), by_slash: HashMap::new(), focus: None, epoch: 0 }
    }
    /// Reject duplicate ids, reserved slash/aliases and equal-class key chords.
    pub fn register<C: Component>(&mut self, component: C) -> Result<ComponentToken, &'static str> {
        let declaration = component.declaration();
        for slot in &self.slots {
            if let Some(mounted) = &slot.component {
                let old = mounted.declaration();
                if old.id == declaration.id { return Err("duplicate component id"); }
                if old.bindings.iter().any(|old| declaration.bindings.iter().any(|new|
                    old.class == new.class && old.code == new.code && old.modifiers == new.modifiers
                )) { return Err("duplicate component binding in input class"); }
            }
        }
        for (index, slash) in declaration.slash.iter().enumerate() {
            if self.by_slash.contains_key(slash.0)
                || crate::model::COMMANDS.iter().any(|core| core.0 == slash.0)
                || declaration.slash[..index].iter().any(|old| old.0 == slash.0)
            { return Err("duplicate or reserved slash command"); }
        }
        for (index, binding) in declaration.bindings.iter().enumerate() {
            if declaration.bindings[..index].iter().any(|old|
                old.class == binding.class && old.code == binding.code && old.modifiers == binding.modifiers
            ) { return Err("duplicate component binding in input class"); }
        }
        let token = ComponentToken { slot: self.slots.len(), generation: 0 };
        for interest in declaration.interests {
            let recipients = self.by_topic.entry(interest.topic).or_default();
            if !recipients.contains(&token) { recipients.push(token); }
        }
        for slash in declaration.slash { self.by_slash.insert(slash.0, token); }
        self.slots.push(Slot { generation: 0, component: Some(Box::new(Mounted(component))) });
        Ok(token)
    }
    /// Tombstone a slot and remove every interest; in-flight replies are fenced.
    pub fn unmount(&mut self, token: ComponentToken) {
        let Some(slot) = self.slots.get_mut(token.slot) else { return };
        if slot.generation != token.generation || slot.component.is_none() { return; }
        slot.component = None;
        slot.generation = slot.generation.wrapping_add(1);
        self.by_topic.retain(|_, tokens| { tokens.retain(|to| *to != token); !tokens.is_empty() });
        self.by_slash.retain(|_, to| *to != token);
        if self.focus.is_some_and(|(owner, _)| owner == token) { self.focus = None; }
    }
    fn finish(&mut self, out: &mut Outbox) {
        if let Some((token, class)) = out.focus.take() {
            if let Some(class) = class { self.focus = Some((token, class)); }
            else if self.focus.is_some_and(|(owner, _)| owner == token) { self.focus = None; }
        }
    }
    /// Fan out borrowed JSON once per interested recipient in registration order.
    pub fn wire(&mut self, event: &TopicEvent, core: &CoreView<'_>) -> Outbox {
        let mut out = Outbox::default();
        // Topic buckets retain registration order. Legacy has no invented topic
        // or version: each owner adapter must explicitly recognize its old tag.
        if let Some(topic) = event.topic() {
            if let Some(tokens) = self.by_topic.get(topic) {
                for &token in tokens {
                    let Some(component) = &mut self.slots[token.slot].component else { continue };
                    let interested = component.declaration().interests.iter().any(|interest|
                        interest.topic == topic && Some(interest.version) == event.version()
                        && (interest.variants.is_empty() || event.event()["type"].as_str().is_some_and(|tag| interest.variants.contains(&tag)))
                    );
                    if interested { component.wire(event, core, token, self.epoch, &mut out); }
                }
            }
        } else {
            for (index, slot) in self.slots.iter_mut().enumerate() {
                if let Some(component) = &mut slot.component {
                    component.wire(event, core, ComponentToken { slot: index, generation: slot.generation }, self.epoch, &mut out);
                }
            }
        }
        self.finish(&mut out);
        out
    }
    /// Drop stale/removed/wrong-owner replies before the typed adapter sees them.
    pub fn reply(&mut self, reply: LocalReply, core: &CoreView<'_>) -> Outbox {
        let mut out = Outbox::default();
        if reply.engine_epoch == self.epoch
            && let Some(slot) = self.slots.get_mut(reply.to.slot)
            && slot.generation == reply.to.generation
            && let Some(component) = &mut slot.component
        { component.reply(reply, core, &mut out); }
        self.finish(&mut out);
        out
    }
    /// Broadcast lifecycle state; engine replacement advances the reply fence.
    pub fn lifecycle(&mut self, event: Lifecycle<'_>, core: &CoreView<'_>) -> Outbox {
        if matches!(event, Lifecycle::EngineChanged) {
            self.epoch = self.epoch.wrapping_add(1);
            self.focus = None;
        }
        if matches!(event, Lifecycle::SessionReset | Lifecycle::Shutdown) { self.focus = None; }
        let mut out = Outbox::default();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if let Some(component) = &mut slot.component {
                component.input(Input::Lifecycle(event), core, ComponentToken { slot: index, generation: slot.generation }, self.epoch, &mut out);
            }
        }
        self.finish(&mut out);
        out
    }
    /// Deliver only due deadlines using the existing frame/line-printer loop.
    pub fn tick(&mut self, now_ms: u64, frame: usize, core: &CoreView<'_>) -> Outbox {
        let mut out = Outbox::default();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if let Some(component) = &mut slot.component
                && component.wake().is_some_and(|at| at <= now_ms)
            { component.input(Input::Tick { now_ms, frame }, core, ComponentToken { slot: index, generation: slot.generation }, self.epoch, &mut out); }
        }
        self.finish(&mut out);
        out
    }
    /// Dispatch at one fixed precedence boundary, stopping after consumption.
    pub fn key(&mut self, class: FocusClass, key: &KeyEvent, core: &CoreView<'_>) -> Outbox {
        let mut out = Outbox::default();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let token = ComponentToken { slot: index, generation: slot.generation };
            if let Some(component) = &mut slot.component {
                let focused = self.focus == Some((token, class));
                let bound = component.declaration().bindings.iter().any(|binding|
                    binding.class == class && binding.code == key.code && binding.modifiers == key.modifiers
                );
                if focused || bound { component.input(Input::Key(key), core, token, self.epoch, &mut out); }
                if out.consumed { break; }
            }
        }
        self.finish(&mut out);
        out
    }
    /// Deliver bracketed paste only to the exclusive focus owner.
    pub fn paste(&mut self, text: &str, core: &CoreView<'_>) -> Outbox {
        let mut out = Outbox::default();
        if let Some((token, _)) = self.focus
            && let Some(component) = &mut self.slots[token.slot].component
        { component.input(Input::Paste(text), core, token, self.epoch, &mut out); }
        self.finish(&mut out);
        out
    }
    /// Deliver a submitted line only to the exclusive focus owner.
    pub fn submit(&mut self, text: &str, core: &CoreView<'_>) -> Outbox {
        let mut out = Outbox::default();
        if let Some((token, _)) = self.focus
            && let Some(component) = &mut self.slots[token.slot].component
        { component.input(Input::Submit(text), core, token, self.epoch, &mut out); }
        self.finish(&mut out);
        out
    }
    /// Return None for unknown commands so skill/raw-text fallback stays intact.
    pub fn slash(&mut self, name: &str, args: &str, core: &CoreView<'_>) -> Option<Outbox> {
        let &token = self.by_slash.get(name)?;
        let component = self.slots[token.slot].component.as_mut()?;
        let mut out = Outbox::default();
        component.input(Input::Slash { name, args }, core, token, self.epoch, &mut out);
        self.finish(&mut out);
        Some(out)
    }
    /// Registration-ordered rows shared by completion/help and dictation.
    pub fn slash_specs(&self) -> impl Iterator<Item = &'static SlashSpec> + '_ {
        self.slots.iter().filter_map(|slot| slot.component.as_ref()).flat_map(|component| component.declaration().slash)
    }
    /// Render in registration order; elements own all retained presentation data.
    pub fn views<'a>(&'a self, at: Surface<'a>, cx: &'a ViewCx<'a>) -> impl Iterator<Item = View> + 'a {
        self.slots.iter().filter_map(move |slot| slot.component.as_ref()?.view(at, cx))
    }
    /// Earliest deadline, without a second scheduler or per-component task.
    pub fn wake(&self) -> Option<u64> {
        self.slots.iter().filter_map(|slot| slot.component.as_ref()?.wake()).min()
    }
    /// Whether a component, rather than the chat editor, currently owns input.
    pub fn focused(&self) -> bool { self.focus.is_some() }
}
