use std::collections::HashMap;
use super::*;

struct Slot { generation: u64, component: Option<Box<dyn ErasedComponent>> }
/// The sole built-in composition list. Add a panel here only after deleting its
/// residual enum/reducer/executor/render/print hooks.
pub fn builtin() -> Registry {
    let mut registry = Registry::empty();
    registry.register(update::Update::default()).expect("built-in component registration");

    // Step-11 group t1 (network and prompts): register below, in this block only.
    registry.register(computer::Computer).expect("built-in component registration");
    registry.register(sandboxes::Sandboxes).expect("built-in component registration");
    registry.register(link::Link::default()).expect("built-in component registration");
    registry.register(nodes::Nodes::default()).expect("built-in component registration");
    registry.register(mesh_devices::MeshDevices::default()).expect("built-in component registration");
    registry.register(secrets::Secrets::default()).expect("built-in component registration");

    // Step-11 group t2 (boards, costs and artifacts): register below, in this block only.
    registry.register(tasks::Tasks).expect("built-in component registration");
    registry.register(roster::Roster).expect("built-in component registration");
    registry.register(routines::Routines).expect("built-in component registration");
    registry.register(autoresearch::Autoresearch).expect("built-in component registration");
    registry.register(costs::Costs::default()).expect("built-in component registration");
    registry.register(web_progress::WebProgress::default()).expect("built-in component registration");
    registry.register(double::Double::default()).expect("built-in component registration");

    // Step-11 group t3 (jobs, terminals, speech and search): register below, in this block only.
    registry.register(jobs::Jobs::default()).expect("built-in component registration");
    registry.register(terminals::Terminals::default()).expect("built-in component registration");
    registry.register(broadcast::BroadcastPanel::default()).expect("built-in component registration");
    registry.register(knowledge::Knowledge::default()).expect("built-in component registration");

    // Model-picker panels (after design 194): register below, in this block only.
    registry
}

/// Ordered built-in storage, topic/slash indexes and exclusive input focus.
pub struct Registry {
    slots: Vec<Slot>,
    by_topic: HashMap<&'static str, Vec<ComponentToken>>,
    by_slash: HashMap<&'static str, ComponentToken>,
    by_wrapper: HashMap<&'static str, ComponentToken>,
    focus: Option<(ComponentToken, FocusClass)>,
    epoch: u64,
    interactive: bool,
}
impl Default for Registry { fn default() -> Self { Self::empty() } }
impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry").field("slots", &self.slots.len()).field("epoch", &self.epoch).finish_non_exhaustive()
    }
}
impl Registry {
    #[cfg(test)]
    pub(crate) fn token(&self, id: &str) -> ComponentToken {
        let slot = self.slots.iter().position(|slot|
            slot.component.as_ref().is_some_and(|component| component.declaration().id == id)
        ).expect("registered component");
        ComponentToken { slot, generation: self.slots[slot].generation }
    }
    /// Set presentation mode before mounting components.
    pub fn set_interactive(&mut self, interactive: bool) { self.interactive = interactive; }
    /// Read presentation mode without borrowing the component slots.
    pub fn interactive(&self) -> bool { self.interactive }
    /// Empty shell for fixed composition and isolated contract consumers.
    pub fn empty() -> Self {
        Self { slots: Vec::new(), by_topic: HashMap::new(), by_slash: HashMap::new(), by_wrapper: HashMap::new(), focus: None, epoch: 0, interactive: true }
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
        let tags = component.wrapper_tags();
        for (index, tag) in tags.iter().enumerate() {
            if self.by_wrapper.contains_key(tag) || tags[..index].contains(tag) {
                return Err("duplicate wrapper tag");
            }
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
        for &tag in tags { self.by_wrapper.insert(tag, token); }
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
        self.by_wrapper.retain(|_, owner| *owner != token);
        if self.focus.is_some_and(|(owner, _)| owner == token) { self.focus = None; }
    }
    /// Project by the sole registered protocol-tag owner; an unowned row keeps core rendering.
    pub fn wrapper(&self, row: Wrapper<'_>) -> Option<WrapperView> {
        let token = self.by_wrapper.get(row.tag)?;
        Some(self.slots[token.slot].component.as_ref()?.wrapper(row))
    }
    fn lose_focus(&mut self, core: &CoreView<'_>, out: &mut Outbox) {
        if let Some((token, _)) = self.focus.take()
            && let Some(component) = &mut self.slots[token.slot].component
        {
            component.input(Input::Lifecycle(Lifecycle::FocusLost), core, token, self.epoch, out);
            out.focus = None;
        }
    }
    /// A residual core modal acquired input; close the displaced owner's surface.
    pub fn release_focus(&mut self, core: &CoreView<'_>) -> Outbox {
        let mut out = Outbox::default();
        self.lose_focus(core, &mut out);
        out
    }
    fn finish(&mut self, out: &mut Outbox, core: &CoreView<'_>) {
        if let Some((token, class)) = out.focus.take() {
            if let Some(class) = class {
                if self.focus.is_some_and(|(owner, _)| owner != token) {
                    self.lose_focus(core, out);
                }
                self.focus = Some((token, class));
                out.claimed = Some(class);
            } else if self.focus.is_some_and(|(owner, _)| owner == token) {
                self.focus = None;
            }
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
        self.finish(&mut out, core);
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
        self.finish(&mut out, core);
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
        self.finish(&mut out, core);
        out
    }
    /// Deliver a core-selected stable entry action; only its owner consumes it.
    pub fn entry_action(&mut self, key: &str, operation: EntryOperation, core: &CoreView<'_>) -> Outbox {
        let mut out = Outbox::default();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if let Some(component) = &mut slot.component {
                component.input(Input::EntryAction { key, operation }, core, ComponentToken { slot: index, generation: slot.generation }, self.epoch, &mut out);
                if out.consumed { break; }
            }
        }
        self.finish(&mut out, core);
        out
    }
    /// Whether an owner has a local representation of this stable entry.
    pub fn entry_openable(&self, key: &str) -> bool {
        self.slots.iter().filter_map(|slot| slot.component.as_ref()).any(|component| component.entry_openable(key))
    }
    /// Last replacement and following append rows, matching entry-view composition exactly.
    pub fn entry_layout(&self, key: &str, width: usize) -> (Option<usize>, usize) {
        let mut replacement = None;
        let mut appended = 0;
        for component in self.slots.iter().filter_map(|slot| slot.component.as_ref()) {
            if let Some(height) = component.entry_height(key, width) { replacement = Some(height); appended = 0; }
            appended += component.entry_rows(key);
        }
        (replacement, appended)
    }
    /// Deliver only due deadlines using the existing frame/line-printer loop.
    pub fn tick(&mut self, now_ms: u64, frame: usize, core: &CoreView<'_>) -> Outbox {
        let mut out = Outbox::default();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if let Some(component) = &mut slot.component
                && component.wake().is_some_and(|at| at <= now_ms)
            { component.input(Input::Tick { now_ms, frame }, core, ComponentToken { slot: index, generation: slot.generation }, self.epoch, &mut out); }
        }
        self.finish(&mut out, core);
        out
    }
    /// Dispatch at one fixed precedence boundary, stopping after consumption.
    pub fn key(&mut self, class: FocusClass, key: &KeyEvent, core: &CoreView<'_>) -> Outbox {
        let mut out = Outbox::default();
        // Intrinsic capture precedes exclusive focus and declared chords.
        let blocked = self.focus.is_some_and(|(_, focused_class)| {
            class != focused_class
                && (focused_class == FocusClass::Modal || self.editor().is_some())
                && !matches!(class, FocusClass::Interactive | FocusClass::Spoken)
        });
        'delivery: for pass in 0..3 {
            if pass == 1 && blocked { break; }
            for (index, slot) in self.slots.iter_mut().enumerate() {
                let token = ComponentToken { slot: index, generation: slot.generation };
                if let Some(component) = &mut slot.component {
                    let capture = component.captures(class);
                    let focused = self.focus == Some((token, class));
                    let selected = match pass {
                        0 => capture,
                        1 => !capture && focused,
                        _ => !capture && !focused && component.declaration().bindings.iter().any(|binding|
                            binding.class == class && binding.code == key.code && binding.modifiers == key.modifiers
                        ),
                    };
                    if selected { component.input(Input::Key(key), core, token, self.epoch, &mut out); }
                    if out.consumed { break 'delivery; }
                }
            }
            if pass == 1 && self.focus.is_some_and(|(_, focused)| focused == class) { break; }
        }
        self.finish(&mut out, core);
        out
    }
    /// Deliver bracketed paste to the intrinsic secret capture or exclusive focus owner.
    pub fn paste(&mut self, text: &str, core: &CoreView<'_>) -> Outbox {
        let mut out = Outbox::default();
        if let Some((token, _)) = self.editor_owner()
            && let Some(component) = &mut self.slots[token.slot].component
        { component.input(Input::Paste(text), core, token, self.epoch, &mut out); }
        self.finish(&mut out, core);
        out
    }
    /// Deliver submitted input to the intrinsic secret capture or exclusive focus owner.
    pub fn submit(&mut self, text: &str, core: &CoreView<'_>) -> Outbox {
        let mut out = Outbox::default();
        if let Some((token, _)) = self.editor_owner()
            && let Some(component) = &mut self.slots[token.slot].component
        { component.input(Input::Submit(text), core, token, self.epoch, &mut out); }
        self.finish(&mut out, core);
        out
    }
    /// Return None for unknown commands so skill/raw-text fallback stays intact.
    pub fn slash(&mut self, name: &str, args: &str, core: &CoreView<'_>) -> Option<Outbox> {
        let &token = self.by_slash.get(name)?;
        let component = self.slots[token.slot].component.as_mut()?;
        let mut out = Outbox::default();
        component.input(Input::Slash { name, args }, core, token, self.epoch, &mut out);
        self.finish(&mut out, core);
        Some(out)
    }
    /// Registration-ordered rows shared by completion/help and dictation.
    pub fn slash_specs(&self) -> impl Iterator<Item = &'static SlashSpec> + '_ {
        self.slots.iter().filter_map(|slot| slot.component.as_ref()).flat_map(|component| {
            component.declaration().slash.iter().filter(move |spec| component.slash_visible(spec.0))
        })
    }
    /// Render in registration order; elements own all retained presentation data.
    pub fn views<'a>(&'a self, at: Surface<'a>, cx: &'a ViewCx<'a>) -> impl Iterator<Item = View> + 'a {
        self.slots.iter().enumerate().filter_map(move |(index, slot)| {
            if matches!(at, Surface::Modal)
                && !self.focus.is_some_and(|(token, _)| token.slot == index)
            { return None; }
            let cx = ViewCx { endpoint: Some((ComponentToken { slot: index, generation: slot.generation }, self.epoch)), ..*cx };
            slot.component.as_ref()?.view(at, &cx)
        })
    }
    /// Earliest deadline, without a second scheduler or per-component task.
    pub fn wake(&self) -> Option<u64> {
        self.slots.iter().filter_map(|slot| slot.component.as_ref()?.wake()).min()
    }
    /// Any mounted owner needs the shared animation clock.
    pub fn animating(&self) -> bool {
        self.slots.iter().filter_map(|slot| slot.component.as_ref()).any(|component| component.animating())
    }
    /// Read-only owner query for composition and component integration tests.
    pub fn get<C: Component>(&self) -> Option<&C> {
        self.slots.iter().filter_map(|slot| slot.component.as_ref()).find_map(|component| component.as_any().downcast_ref())
    }
    /// Mechanical fixture rebinding only; production components cannot mutate peers.
    #[cfg(test)]
    pub(crate) fn get_mut<C: Component>(&mut self) -> Option<&mut C> {
        self.slots.iter_mut().filter_map(|slot| slot.component.as_mut()).find_map(|component| component.as_any_mut().downcast_mut())
    }
    #[cfg(test)]
    pub(crate) fn focus_fixture(&mut self, id: &str, class: FocusClass) {
        self.focus = Some((self.token(id), class));
    }
    /// Whether a component, rather than the chat editor, currently owns input.
    pub fn focused(&self) -> bool { self.focus.is_some() }
    fn editor_owner(&self) -> Option<(ComponentToken, FocusClass)> {
        self.slots.iter().enumerate().find_map(|(slot, mounted)| {
            mounted.component.as_ref()?.captures(FocusClass::Secret).then_some((
                ComponentToken { slot, generation: mounted.generation }, FocusClass::Secret,
            ))
        }).or(self.focus)
    }
    /// Composer configuration at secret precedence, otherwise the exclusive focus owner.
    pub fn editor(&self) -> Option<(FocusClass, Editor)> {
        let (token, class) = self.editor_owner()?;
        Some((class, self.slots[token.slot].component.as_ref()?.editor()?))
    }
    /// Modal capture excludes composer-backed prompts, just like the core prompt.
    pub fn modal(&self) -> bool {
        self.focus.is_some_and(|(_, class)| class == FocusClass::Modal) && self.editor().is_none()
    }
}
