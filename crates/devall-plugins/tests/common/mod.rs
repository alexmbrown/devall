//! Shared test helpers: [`Wat`] generates a plugin module implementing the ABI (a bump `alloc` and
//! a `devall_call` that dispatches on the envelope's `kind` to canned behaviours).
#![allow(dead_code)]

/// What `devall_call` does for one `kind`.
#[derive(Clone)]
pub enum Act {
    /// Return this exact document.
    Const(String),
    /// `{"ok":{"output":<the whole call envelope>}}`.
    Echo,
    /// Spin forever.
    Loop,
    /// Grow memory until refused, then trap.
    Grow,
    /// Call `devall.host_call` with this JSON and return `prefix + reply + suffix`.
    Host { call: String, prefix: String, suffix: String },
    /// Call `devall.host_call`, ignore the reply and report an error of its own.
    HostThenFail(String),
    /// Log a line, then return this document.
    LogThen(String, String),
}

pub struct Wat {
    data: Vec<(u32, Vec<u8>)>,
    next: u32,
    pub extra_imports: String,
    pub memory_pages: u32,
}

const KIND_AT: usize = r#"{"apiVersion":1,"kind":""#.len();

fn esc(b: &[u8]) -> String {
    b.iter().map(|c| format!("\\{c:02x}")).collect()
}

impl Default for Wat {
    fn default() -> Self {
        Self::new()
    }
}

impl Wat {
    pub fn new() -> Self {
        Self { data: Vec::new(), next: 1024, extra_imports: String::new(), memory_pages: 2 }
    }

    fn lit(&mut self, s: &str) -> (u32, u32) {
        let off = self.next;
        self.data.push((off, s.as_bytes().to_vec()));
        self.next += s.len() as u32 + 1;
        assert!(self.next < 60_000, "literals overflow page 0");
        (off, s.len() as u32)
    }

    fn emit(&mut self, s: &str) -> String {
        let (o, l) = self.lit(s);
        format!("(call $emit (i32.const {o}) (i32.const {l}))\n")
    }

    fn act(&mut self, a: &Act) -> String {
        match a {
            Act::Const(s) => self.emit(s),
            Act::Echo => {
                let p = self.emit(r#"{"ok":{"output":"#);
                let s = self.emit("}}");
                format!("{p}(call $emit (local.get $ptr) (local.get $len))\n{s}")
            }
            Act::Loop => "(loop $spin (br $spin))\n".into(),
            Act::Grow => "(loop $g (if (i32.eq (memory.grow (i32.const 16)) (i32.const -1)) (then unreachable)) (br $g))\n".into(),
            Act::Host { call, prefix, suffix } => {
                let (co, cl) = self.lit(call);
                let p = self.emit(prefix);
                let s = self.emit(suffix);
                format!(
                    "(local.set $r (call $host (i32.const {co}) (i32.const {cl})))\n{p}(call $emit (i32.wrap_i64 (i64.shr_u (local.get $r) (i64.const 32))) (i32.wrap_i64 (local.get $r)))\n{s}"
                )
            }
            Act::HostThenFail(call) => {
                let (co, cl) = self.lit(call);
                format!("(local.set $r (call $host (i32.const {co}) (i32.const {cl})))
{}", self.emit(r#"{"error":"something went wrong"}"#))
            }
            Act::LogThen(line, out) => {
                let (lo, ll) = self.lit(line);
                format!("(call $log (i32.const 1) (i32.const {lo}) (i32.const {ll}))\n{}", self.emit(out))
            }
        }
    }

    /// A module answering `cases` (kind → act; `""` matches anything).
    pub fn module(mut self, cases: &[(&str, Act)]) -> Vec<u8> {
        let mut body = String::new();
        for (kind, act) in cases {
            let code = self.act(act);
            if kind.is_empty() {
                body.push_str(&code);
                body.push_str("(return (call $packed))\n");
            } else {
                let (ko, kl) = self.lit(&format!("{kind}\""));
                body.push_str(&format!(
                    "(if (call $starts (i32.add (local.get $ptr) (i32.const {KIND_AT})) (i32.const {ko}) (i32.const {kl})) (then\n{code}(return (call $packed))))\n"
                ));
            }
        }
        body.push_str(&self.emit(r#"{"error":"unhandled kind"}"#));
        body.push_str("(call $packed)\n");
        let data: String = self.data.iter().map(|(o, b)| format!("(data (i32.const {o}) \"{}\")\n", esc(b))).collect();
        let text = format!(
            r#"(module
  (import "devall" "host_call" (func $host (param i32 i32) (result i64)))
  (import "devall" "log" (func $log (param i32 i32 i32)))
  {extra}
  (memory (export "memory") {pages})
  (global $heap (mut i32) (i32.const 65536))
  (global $out (mut i32) (i32.const 0))
  (global $olen (mut i32) (i32.const 0))
  {data}
  (func $alloc (export "alloc") (param $len i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $heap))
    (block $ok
      (loop $grow
        (br_if $ok (i32.le_u (i32.add (local.get $p) (local.get $len)) (i32.mul (memory.size) (i32.const 65536))))
        (if (i32.eq (memory.grow (i32.const 1)) (i32.const -1)) (then unreachable))
        (br $grow)))
    (global.set $heap (i32.add (local.get $p) (local.get $len)))
    (local.get $p))
  (func (export "dealloc") (param i32 i32))
  (func $emit (param $src i32) (param $n i32)
    (memory.copy (i32.add (global.get $out) (global.get $olen)) (local.get $src) (local.get $n))
    (global.set $olen (i32.add (global.get $olen) (local.get $n))))
  (func $packed (result i64)
    (i64.or (i64.shl (i64.extend_i32_u (global.get $out)) (i64.const 32)) (i64.extend_i32_u (global.get $olen))))
  (func $starts (param $p i32) (param $l i32) (param $n i32) (result i32)
    (local $i i32)
    (block $no
      (loop $c
        (if (i32.ge_u (local.get $i) (local.get $n)) (then (return (i32.const 1))))
        (br_if $no (i32.ne (i32.load8_u (i32.add (local.get $p) (local.get $i))) (i32.load8_u (i32.add (local.get $l) (local.get $i)))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $c)))
    (i32.const 0))
  (func (export "devall_call") (param $ptr i32) (param $len i32) (result i64)
    (local $r i64)
    (global.set $out (call $alloc (i32.add (local.get $len) (i32.const 16384))))
    (global.set $olen (i32.const 0))
    {body}))"#,
            extra = self.extra_imports,
            pages = self.memory_pages,
        );
        wat::parse_str(&text).unwrap_or_else(|e| panic!("bad WAT: {e}\n{text}"))
    }
}

pub fn module(cases: &[(&str, Act)]) -> Vec<u8> {
    Wat::new().module(cases)
}

