//! Merges parsed configs in load order and resolves class bases and inherited members.

use crate::ast::{ConfigClass, ConfigFile, ConfigProperty, Member, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

type NodeId = usize;
const ROOT: NodeId = 0;

/// Where a source config comes from; decides load-order ties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigOrigin {
    Vanilla,
    Dependency(u16),
    Workspace,
}

/// One parsed config plus the identity the rest of the app knows it by.
#[derive(Debug, Clone)]
pub struct SourceConfig {
    pub id: String,
    pub display: String,
    pub vpath: String,
    pub origin: ConfigOrigin,
    pub file: ConfigFile,
}

/// One `CfgPatches` class and the source that declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchInfo {
    pub name: String,
    pub source: String,
    pub required_addons: Vec<String>,
}

/// A class in the merged tree, flattened over its base chain.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedClass {
    pub path: Vec<String>,
    pub base_chain: Vec<BaseLink>,
    pub properties: Vec<ResolvedProperty>,
    pub children: Vec<ResolvedChild>,
    pub rebases: Vec<Rebase>,
    pub unresolved_base: Option<String>,
}

/// One class on a base chain and the source that defines it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseLink {
    pub path: Vec<String>,
    pub source: String,
}

/// A property with the class and source it physically comes from.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedProperty {
    pub name: String,
    pub value: Value,
    pub array: bool,
    pub from_class: Vec<String>,
    pub from_source: String,
    pub own: bool,
}

/// A child class, own or inherited, with its descendant count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChild {
    pub name: String,
    pub own: bool,
    pub from_class: Vec<String>,
    pub classes_below: usize,
}

/// A later full definition that changed a class's base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rebase {
    pub source: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Copy)]
enum BaseState {
    Pending,
    Visiting,
    NoBase,
    Found(NodeId),
    Missing,
}

#[derive(Debug, Clone)]
struct Prop {
    name: String,
    value: Value,
    array: bool,
    source: usize,
    appended: bool,
}

#[derive(Debug, Clone)]
struct Node {
    name: String,
    parent: Option<NodeId>,
    base: Option<String>,
    children: Vec<NodeId>,
    child_index: HashMap<String, NodeId>,
    props: Vec<Prop>,
    prop_index: HashMap<String, usize>,
    defined_by: Vec<usize>,
    created_by: usize,
    rebases: Vec<(usize, String, String)>,
}

impl Node {
    fn new(name: &str, parent: Option<NodeId>, created_by: usize) -> Self {
        Self {
            name: name.to_owned(),
            parent,
            base: None,
            children: Vec::new(),
            child_index: HashMap::new(),
            props: Vec::new(),
            prop_index: HashMap::new(),
            defined_by: Vec::new(),
            created_by,
            rebases: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct PatchDecl {
    name: String,
    required: Vec<String>,
}

/// The merged view of a set of configs; built once, then queried.
#[derive(Debug, Clone)]
pub struct Resolver {
    sources: Vec<Arc<SourceConfig>>,
    by_id: HashMap<String, usize>,
    order: Vec<usize>,
    cycles: Vec<Vec<usize>>,
    decls: Vec<Vec<PatchDecl>>,
    dependencies: Vec<Vec<usize>>,
    nodes: Vec<Node>,
    states: Vec<BaseState>,
}

fn lower(name: &str) -> String {
    name.to_ascii_lowercase()
}

impl Resolver {
    /// Computes the load order and the merged class tree.
    pub fn new(sources: Vec<SourceConfig>) -> Self {
        Self::from_shared(sources.into_iter().map(Arc::new).collect())
    }

    /// [`Resolver::new`] over sources the caller already shares.
    pub fn from_shared(sources: Vec<Arc<SourceConfig>>) -> Self {
        Self::build(sources)
    }

    /// Source ids in load order.
    pub fn load_order(&self) -> Vec<&str> {
        self.order
            .iter()
            .map(|&i| self.sources[i].id.as_str())
            .collect()
    }

    /// `requiredAddons` cycles as source ids in load-order sequence.
    pub fn cycles(&self) -> Vec<Vec<String>> {
        self.cycles
            .iter()
            .map(|c| c.iter().map(|&i| self.sources[i].id.clone()).collect())
            .collect()
    }

    /// Every `CfgPatches` class in load order.
    pub fn patches(&self) -> Vec<PatchInfo> {
        self.order
            .iter()
            .flat_map(|&i| {
                self.decls[i].iter().map(move |d| PatchInfo {
                    name: d.name.clone(),
                    source: self.sources[i].id.clone(),
                    required_addons: d.required.clone(),
                })
            })
            .collect()
    }

    /// Every `requiredAddons` name a vanilla source lists, lowercased.
    pub fn vanilla_required_addons(&self) -> HashSet<String> {
        self.sources
            .iter()
            .zip(&self.decls)
            .filter(|(source, _)| source.origin == ConfigOrigin::Vanilla)
            .flat_map(|(_, decls)| decls.iter().flat_map(|decl| &decl.required))
            .map(|name| lower(name))
            .collect()
    }

    /// The first `CfgPatches` class a source declares.
    pub fn owner_patch(&self, source_id: &str) -> Option<&str> {
        let i = *self.by_id.get(source_id)?;
        self.decls[i].first().map(|d| d.name.as_str())
    }

    /// Sources reachable through `requiredAddons`, plus itself and every source without `CfgPatches`.
    pub fn required_closure(&self, source_id: &str) -> HashSet<String> {
        let mut seen: HashSet<usize> = (0..self.sources.len())
            .filter(|&i| self.decls[i].is_empty())
            .collect();
        let mut stack: Vec<usize> = self.by_id.get(source_id).copied().into_iter().collect();
        let mut visited = HashSet::new();
        while let Some(i) = stack.pop() {
            if visited.insert(i) {
                stack.extend(&self.dependencies[i]);
            }
        }
        seen.extend(visited);
        seen.into_iter()
            .map(|i| self.sources[i].id.clone())
            .collect()
    }

    /// The class flattened over its base chain, or `None` when the path does not exist.
    pub fn class(&self, path: &[String]) -> Option<ResolvedClass> {
        let (id, path) = self.locate(path)?;
        let chain = self.chain(id);
        let paths: Vec<Vec<String>> = chain.iter().map(|&n| self.node_path(n)).collect();
        let (properties, children) = self.overlay(id, &chain, &paths);
        let last = *chain.last().unwrap_or(&id);
        let unresolved_base = match self.states[last] {
            BaseState::Missing => self.nodes[last].base.clone(),
            _ => None,
        };
        Some(ResolvedClass {
            path,
            base_chain: chain
                .iter()
                .zip(&paths)
                .skip(1)
                .map(|(&n, p)| BaseLink {
                    path: p.clone(),
                    source: self.sources[self.link_source(n)].id.clone(),
                })
                .collect(),
            properties,
            children,
            rebases: self.nodes[id]
                .rebases
                .iter()
                .map(|(s, from, to)| Rebase {
                    source: self.sources[*s].id.clone(),
                    from: from.clone(),
                    to: to.clone(),
                })
                .collect(),
            unresolved_base,
        })
    }

    /// Sources that define the class with a body, in load order.
    pub fn defined_by(&self, path: &[String]) -> Vec<&str> {
        let mut cur = ROOT;
        for segment in path {
            match self.nodes[cur].child_index.get(&lower(segment)) {
                Some(&c) => cur = c,
                None => return Vec::new(),
            }
        }
        if path.is_empty() {
            return Vec::new();
        }
        self.nodes[cur]
            .defined_by
            .iter()
            .map(|&i| self.sources[i].id.as_str())
            .collect()
    }

    /// A resolver with the source of the same id swapped for `source` (or added).
    pub fn with_overlay(&self, source: SourceConfig) -> Resolver {
        let mut sources = self.sources.clone();
        match self.by_id.get(&source.id) {
            Some(&i) => sources[i] = Arc::new(source),
            None => sources.push(Arc::new(source)),
        }
        Self::build(sources)
    }

    fn build(sources: Vec<Arc<SourceConfig>>) -> Self {
        let by_id = sources
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.clone(), i))
            .collect();
        let decls: Vec<Vec<PatchDecl>> = sources.iter().map(|s| patch_decls(&s.file)).collect();
        let dependencies = dependency_edges(&decls);
        let by_key = sorted_by_load_key(&sources);
        let order = load_order(&by_key, &decls, &dependencies);
        let cycles = find_cycles(&by_key, &decls, &dependencies);

        let mut nodes = vec![Node::new("", None, 0)];
        for &i in &order {
            merge_members(&mut nodes, ROOT, &sources[i].file.members, i);
        }
        let mut states = vec![BaseState::Pending; nodes.len()];
        for id in 0..nodes.len() {
            resolve_base(&nodes, &mut states, id);
        }
        Self {
            sources,
            by_id,
            order,
            cycles,
            decls,
            dependencies,
            nodes,
            states,
        }
    }

    fn locate(&self, path: &[String]) -> Option<(NodeId, Vec<String>)> {
        if path.is_empty() {
            return None;
        }
        let mut cur = ROOT;
        let mut names = Vec::with_capacity(path.len());
        for segment in path {
            cur = find_child(&self.nodes, cur, &lower(segment), None, &mut |n| {
                self.states[n]
            })?;
            names.push(self.nodes[cur].name.clone());
        }
        Some((cur, names))
    }

    fn chain(&self, id: NodeId) -> Vec<NodeId> {
        let mut chain = vec![id];
        let mut seen = HashSet::from([id]);
        while let BaseState::Found(next) = self.states[*chain.last().unwrap_or(&id)] {
            if !seen.insert(next) {
                break;
            }
            chain.push(next);
        }
        chain
    }

    fn node_path(&self, id: NodeId) -> Vec<String> {
        let mut names = Vec::new();
        let mut cur = id;
        while let Some(parent) = self.nodes[cur].parent {
            names.push(self.nodes[cur].name.clone());
            cur = parent;
        }
        names.reverse();
        names
    }

    fn link_source(&self, id: NodeId) -> usize {
        let node = &self.nodes[id];
        node.defined_by.first().copied().unwrap_or(node.created_by)
    }

    fn overlay(
        &self,
        target: NodeId,
        chain: &[NodeId],
        paths: &[Vec<String>],
    ) -> (Vec<ResolvedProperty>, Vec<ResolvedChild>) {
        let mut properties: Vec<ResolvedProperty> = Vec::new();
        let mut prop_index: HashMap<String, usize> = HashMap::new();
        let mut children: Vec<ResolvedChild> = Vec::new();
        let mut child_index: HashMap<String, usize> = HashMap::new();
        for (&n, path) in chain.iter().zip(paths).rev() {
            for p in &self.nodes[n].props {
                let key = lower(&p.name);
                let previous = prop_index.get(&key).copied();
                let value = match previous {
                    Some(i) if p.appended => concat(&properties[i].value, &p.value),
                    _ => p.value.clone(),
                };
                let resolved = ResolvedProperty {
                    name: p.name.clone(),
                    value,
                    array: p.array,
                    from_class: path.clone(),
                    from_source: self.sources[p.source].id.clone(),
                    own: n == target,
                };
                match previous {
                    Some(i) => properties[i] = resolved,
                    None => {
                        prop_index.insert(key, properties.len());
                        properties.push(resolved);
                    }
                }
            }
            for &c in &self.nodes[n].children {
                let child = &self.nodes[c];
                let resolved = ResolvedChild {
                    name: child.name.clone(),
                    own: n == target,
                    from_class: path.clone(),
                    classes_below: self.classes_below(c),
                };
                match child_index.get(&lower(&child.name)).copied() {
                    Some(i) => children[i] = resolved,
                    None => {
                        child_index.insert(lower(&child.name), children.len());
                        children.push(resolved);
                    }
                }
            }
        }
        (properties, children)
    }

    fn classes_below(&self, id: NodeId) -> usize {
        self.nodes[id]
            .children
            .iter()
            .map(|&c| 1 + self.classes_below(c))
            .sum()
    }
}

fn concat(existing: &Value, appended: &Value) -> Value {
    match (existing, appended) {
        (Value::Array(a), Value::Array(b)) => Value::Array(a.iter().chain(b).cloned().collect()),
        _ => appended.clone(),
    }
}

fn merge_members(nodes: &mut Vec<Node>, parent: NodeId, members: &[Member], source: usize) {
    for member in members {
        match member {
            Member::Class(class) => merge_class(nodes, parent, class, source),
            Member::Property(prop) => merge_property(&mut nodes[parent], prop, source),
            Member::Delete { name, .. } => delete_member(nodes, parent, name),
        }
    }
}

fn merge_class(nodes: &mut Vec<Node>, parent: NodeId, class: &ConfigClass, source: usize) {
    let key = lower(&class.name);
    let id = match nodes[parent].child_index.get(&key) {
        Some(&id) => id,
        None => {
            let id = nodes.len();
            nodes.push(Node::new(&class.name, Some(parent), source));
            nodes[parent].children.push(id);
            nodes[parent].child_index.insert(key, id);
            id
        }
    };
    let base = class.parents.first();
    if class.forward {
        if nodes[id].base.is_none() {
            nodes[id].base = base.cloned();
        }
        return;
    }
    if let Some(new) = base {
        match nodes[id].base.clone() {
            Some(old) if !old.eq_ignore_ascii_case(new) => {
                nodes[id].rebases.push((source, old, new.clone()));
                nodes[id].base = Some(new.clone());
            }
            Some(_) => {}
            None => nodes[id].base = Some(new.clone()),
        }
    }
    if !nodes[id].defined_by.contains(&source) {
        nodes[id].defined_by.push(source);
    }
    merge_members(nodes, id, &class.members, source);
}

fn merge_property(node: &mut Node, prop: &ConfigProperty, source: usize) {
    let key = lower(&prop.name);
    let value = prop.value.clone().unwrap_or(if prop.declared_array {
        Value::Array(Vec::new())
    } else {
        Value::Other(String::new())
    });
    let array = prop.declared_array || matches!(value, Value::Array(_));
    match node.prop_index.get(&key).copied() {
        Some(i) if prop.array_append => {
            let existing = &mut node.props[i];
            existing.value = concat(&existing.value, &value);
            existing.source = source;
        }
        Some(i) => {
            node.props[i] = Prop {
                name: prop.name.clone(),
                value,
                array,
                source,
                appended: false,
            }
        }
        None => {
            node.prop_index.insert(key, node.props.len());
            node.props.push(Prop {
                name: prop.name.clone(),
                value,
                array,
                source,
                appended: prop.array_append,
            });
        }
    }
}

fn delete_member(nodes: &mut [Node], parent: NodeId, name: &str) {
    let key = lower(name);
    let node = &mut nodes[parent];
    if let Some(i) = node.prop_index.remove(&key) {
        node.props.remove(i);
        node.prop_index = node
            .props
            .iter()
            .enumerate()
            .map(|(i, p)| (lower(&p.name), i))
            .collect();
    }
    if let Some(child) = node.child_index.remove(&key) {
        node.children.retain(|&c| c != child);
    }
}

fn resolve_base(nodes: &[Node], states: &mut [BaseState], id: NodeId) -> BaseState {
    match states[id] {
        BaseState::Pending => {}
        BaseState::Visiting => return BaseState::Missing,
        settled => return settled,
    }
    states[id] = BaseState::Visiting;
    let result = match &nodes[id].base {
        None => BaseState::NoBase,
        Some(base) => {
            let key = lower(base);
            let mut scope = nodes[id].parent;
            let mut found = BaseState::Missing;
            while let Some(s) = scope {
                if let Some(t) = find_child(nodes, s, &key, Some(id), &mut |n| {
                    resolve_base(nodes, states, n)
                }) {
                    found = BaseState::Found(t);
                    break;
                }
                scope = nodes[s].parent;
            }
            found
        }
    };
    states[id] = result;
    result
}

/// Looks `key` up among `start`'s own children, then among those it inherits through its base chain.
fn find_child(
    nodes: &[Node],
    start: NodeId,
    key: &str,
    exclude: Option<NodeId>,
    base_of: &mut dyn FnMut(NodeId) -> BaseState,
) -> Option<NodeId> {
    let hit = |n: NodeId| {
        nodes[n]
            .child_index
            .get(key)
            .copied()
            .filter(|&c| Some(c) != exclude)
    };
    if let Some(c) = hit(start) {
        return Some(c);
    }
    let mut seen = HashSet::from([start]);
    let mut cur = start;
    while let BaseState::Found(next) = base_of(cur) {
        if !seen.insert(next) {
            break;
        }
        if let Some(c) = hit(next) {
            return Some(c);
        }
        cur = next;
    }
    None
}

fn patch_decls(file: &ConfigFile) -> Vec<PatchDecl> {
    file.members
        .iter()
        .filter_map(|m| match m {
            Member::Class(c) if c.name.eq_ignore_ascii_case("CfgPatches") => Some(c),
            _ => None,
        })
        .flat_map(|patches| &patches.members)
        .filter_map(|m| match m {
            Member::Class(patch) => Some(PatchDecl {
                name: patch.name.clone(),
                required: required_addons(patch),
            }),
            _ => None,
        })
        .collect()
}

fn required_addons(patch: &ConfigClass) -> Vec<String> {
    crate::ast::required_addon_properties(patch)
        .filter_map(|p| match p.value.as_ref() {
            Some(Value::Array(items)) => Some(items),
            _ => None,
        })
        .flatten()
        .filter_map(|v| match v {
            Value::Str(s) | Value::Ident(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn dependency_edges(decls: &[Vec<PatchDecl>]) -> Vec<Vec<usize>> {
    let mut owners: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, source) in decls.iter().enumerate() {
        for decl in source {
            owners.entry(lower(&decl.name)).or_default().push(i);
        }
    }
    decls
        .iter()
        .enumerate()
        .map(|(i, source)| {
            let deps: BTreeSet<usize> = source
                .iter()
                .flat_map(|d| &d.required)
                .filter_map(|r| owners.get(&lower(r)))
                .flatten()
                .copied()
                .filter(|&d| d != i)
                .collect();
            deps.into_iter().collect()
        })
        .collect()
}

fn sorted_by_load_key(sources: &[Arc<SourceConfig>]) -> Vec<usize> {
    let key = |i: usize| {
        let s = &sources[i];
        let (rank, n) = match s.origin {
            ConfigOrigin::Vanilla => (0u8, 0u16),
            ConfigOrigin::Dependency(n) => (1, n),
            ConfigOrigin::Workspace => (2, 0),
        };
        (rank, n, s.id.as_str())
    };
    let mut indices: Vec<usize> = (0..sources.len()).collect();
    indices.sort_by(|&a, &b| key(a).cmp(&key(b)));
    indices
}

fn load_order(by_key: &[usize], decls: &[Vec<PatchDecl>], deps: &[Vec<usize>]) -> Vec<usize> {
    let mut order: Vec<usize> = by_key
        .iter()
        .copied()
        .filter(|&i| decls[i].is_empty())
        .collect();
    let rank: HashMap<usize, usize> = by_key.iter().enumerate().map(|(r, &i)| (i, r)).collect();
    let mut pending: HashMap<usize, usize> = HashMap::new();
    let mut dependents: HashMap<usize, Vec<usize>> = HashMap::new();
    for &i in by_key.iter().filter(|&&i| !decls[i].is_empty()) {
        pending.insert(i, deps[i].len());
        for &d in &deps[i] {
            dependents.entry(d).or_default().push(i);
        }
    }
    let mut remaining: BTreeSet<usize> = pending.keys().map(|i| rank[i]).collect();
    let mut ready: BTreeSet<usize> = remaining
        .iter()
        .copied()
        .filter(|&r| pending[&by_key[r]] == 0)
        .collect();
    while let Some(next) = ready.pop_first().or_else(|| remaining.first().copied()) {
        if !remaining.remove(&next) {
            continue;
        }
        let i = by_key[next];
        order.push(i);
        for &waiting in dependents.get(&i).into_iter().flatten() {
            let count = pending.entry(waiting).or_default();
            *count = count.saturating_sub(1);
            if *count == 0 && remaining.contains(&rank[&waiting]) {
                ready.insert(rank[&waiting]);
            }
        }
    }
    order
}

fn find_cycles(by_key: &[usize], decls: &[Vec<PatchDecl>], deps: &[Vec<usize>]) -> Vec<Vec<usize>> {
    struct Tarjan<'a> {
        deps: &'a [Vec<usize>],
        index: HashMap<usize, usize>,
        low: HashMap<usize, usize>,
        stack: Vec<usize>,
        on_stack: HashSet<usize>,
        components: Vec<Vec<usize>>,
    }
    impl Tarjan<'_> {
        fn visit(&mut self, v: usize) {
            let n = self.index.len();
            self.index.insert(v, n);
            self.low.insert(v, n);
            self.stack.push(v);
            self.on_stack.insert(v);
            for &w in &self.deps[v] {
                if !self.index.contains_key(&w) {
                    self.visit(w);
                    let low = self.low[&v].min(self.low[&w]);
                    self.low.insert(v, low);
                } else if self.on_stack.contains(&w) {
                    let low = self.low[&v].min(self.index[&w]);
                    self.low.insert(v, low);
                }
            }
            if self.low[&v] == self.index[&v] {
                let mut component = Vec::new();
                while let Some(w) = self.stack.pop() {
                    self.on_stack.remove(&w);
                    component.push(w);
                    if w == v {
                        break;
                    }
                }
                self.components.push(component);
            }
        }
    }
    let rank: HashMap<usize, usize> = by_key.iter().enumerate().map(|(r, &i)| (i, r)).collect();
    let mut tarjan = Tarjan {
        deps,
        index: HashMap::new(),
        low: HashMap::new(),
        stack: Vec::new(),
        on_stack: HashSet::new(),
        components: Vec::new(),
    };
    for &i in by_key.iter().filter(|&&i| !decls[i].is_empty()) {
        if !tarjan.index.contains_key(&i) {
            tarjan.visit(i);
        }
    }
    let mut cycles: Vec<Vec<usize>> = tarjan
        .components
        .into_iter()
        .filter(|c| c.len() > 1)
        .map(|mut c| {
            c.sort_by_key(|i| rank[i]);
            c
        })
        .collect();
    cycles.sort_by_key(|c| rank[&c[0]]);
    cycles
}
