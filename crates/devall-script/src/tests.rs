use std::time::{Duration, Instant};

use base64::Engine as _;
use devall_core::vars::SecretLookup;
use devall_core::{Body, FolderFile, KeyValue, Project, Scripts};
use sha2::Digest;

use super::*;

fn vars(pairs: &[(&str, &str)]) -> Variables {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn request() -> RequestView {
    RequestView {
        method: "GET".into(),
        url: "https://api.example.com/users".into(),
        query: vec![("page".into(), "1".into())],
        headers: vec![("Accept".into(), "application/json".into()), ("X-Trace".into(), "abc".into())],
        body: None,
    }
}

const BODY: &str = r#"{"id":7,"name":"Ada ✓","tags":["a","b"],"nested":{"token":"t0k3n","list":[{"v":1},{"v":2}]}}"#;

fn response() -> ResponseInfo {
    let headers = vec![
        ("Content-Type".to_string(), "application/json; charset=utf-8".to_string()),
        ("Set-Cookie".to_string(), "session=xyz; Path=/; HttpOnly".to_string()),
        ("X-Rate".to_string(), "10".to_string()),
    ];
    ResponseInfo::new(200, "OK", &headers, BODY.as_bytes(), 42.5, BODY.len())
}

fn info() -> ScriptInfo {
    ScriptInfo {
        request_name: "Get user".into(),
        request_id: "01ABC".into(),
        request_path: "users/get-user.req.yaml".into(),
        iteration: 0,
        iteration_count: 1,
        environment_name: Some("dev".into()),
    }
}

fn layered_vars() -> ScriptVars {
    ScriptVars {
        runtime: vars(&[("token", "rt"), ("nested", "{{user}}-x")]),
        iteration_data: vars(&[("user", "ada"), ("row", "1")]),
        environment: vars(&[("baseUrl", "https://env.example.com"), ("token", "env-token"), ("envOnly", "e")]),
        folders: vec![vars(&[("folderVar", "near"), ("shared", "near")]), vars(&[("shared", "far"), ("outer", "o")])],
        project: vars(&[("projectVar", "p"), ("shared", "project")]),
    }
}

fn pre_input() -> PreRequestInput {
    PreRequestInput { request: request(), vars: ScriptVars::default(), info: info(), secrets: Variables::new() }
}

fn post_input() -> PostResponseInput {
    PostResponseInput { request: request(), response: response(), vars: ScriptVars::default(), info: info(), secrets: Variables::new() }
}

fn src(code: &str) -> [ScriptSource; 1] {
    [ScriptSource::new("request", code)]
}

fn run_pre(code: &str) -> ScriptOutput {
    ScriptEngine::new().run_pre_request(&src(code), &pre_input())
}

fn run_post(code: &str) -> ScriptOutput {
    ScriptEngine::new().run_post_response(&src(code), &post_input())
}

#[track_caller]
fn ok(o: &ScriptOutput) {
    assert!(o.error.is_none(), "unexpected error: {:?}\nlogs: {:#?}", o.error, o.logs);
}

fn logs(o: &ScriptOutput) -> Vec<&str> {
    o.logs.iter().map(|l| l.message.as_str()).collect()
}

#[track_caller]
fn test_named<'a>(o: &'a ScriptOutput, name: &str) -> &'a ScriptTestResult {
    o.tests.iter().find(|t| t.label == name).unwrap_or_else(|| panic!("no test {name:?} in {:#?}", o.tests))
}

// ---------------------------------------------------------------- console

#[test]
fn console_capture_and_formatting() {
    let o = run_pre(
        r#"
        console.log('hello', 42, true, null, undefined);
        console.info({ a: 1, b: 'x', c: [1, 2, { d: null }] });
        console.warn('%s is %d years, %i%%', 'Ada', 36.7, 36.7);
        console.error(new Error('bad'));
        console.log([1, 'two', { three: 3 }]);
        console.log({ deep: { er: { est: { x: 1 } } } });
        console.debug('dbg');
        console.log('héllo ✓');
        console.log(new Map([['k', 1]]), new Set([1]), new Date(0), /re/g);
        console.log(pm.request.headers, pm.request.url);
        "#,
    );
    ok(&o);
    assert_eq!(
        logs(&o),
        [
            "hello 42 true null undefined",
            "{ a: 1, b: 'x', c: [ 1, 2, { d: null } ] }",
            "Ada is 36.7 years, 36%",
            "Error: bad",
            "[ 1, 'two', { three: 3 } ]",
            "{ deep: { er: { est: { x: 1 } } } }",
            "dbg",
            "héllo ✓",
            "Map(1) { 'k' => 1 } Set(1) { 1 } 1970-01-01T00:00:00.000Z /re/g",
            "{ Accept: 'application/json', 'X-Trace': 'abc' } 'https://api.example.com/users?page=1'",
        ]
    );
    let levels: Vec<LogLevel> = o.logs.iter().map(|l| l.level).collect();
    assert_eq!(&levels[..4], &[LogLevel::Log, LogLevel::Info, LogLevel::Warn, LogLevel::Error]);
    assert_eq!(levels[6], LogLevel::Debug);
    assert!(o.logs.iter().all(|l| l.source == "request" && l.phase == ScriptPhase::PreRequest));
}

#[test]
fn console_output_is_capped() {
    let o = run_pre("for (let i = 0; i < 1500; i++) console.log(i);");
    ok(&o);
    assert_eq!(o.logs.len(), 1001);
    assert!(o.logs[1000].message.contains("500 more console line(s) dropped"), "{:?}", o.logs[1000]);
    assert_eq!(o.logs[1000].level, LogLevel::Warn);
}

// ---------------------------------------------------------------- variables

#[test]
fn variable_scopes_and_precedence() {
    let input = PreRequestInput { vars: layered_vars(), ..pre_input() };
    let o = ScriptEngine::new().run_pre_request(
        &src(
            r#"
            console.log(pm.variables.get('token'), pm.variables.get('user'), pm.variables.get('baseUrl'), pm.variables.get('shared'), pm.variables.get('projectVar'), pm.variables.get('nope'));
            console.log(pm.environment.get('envOnly'), pm.environment.get('token'), pm.environment.get('projectVar'), pm.environment.has('envOnly'), pm.environment.name);
            console.log(pm.collectionVariables.get('shared'), pm.collectionVariables.get('outer'), pm.collectionVariables.get('baseUrl'));
            console.log(pm.globals.get('token'), pm.globals.get('baseUrl'));
            console.log(pm.iterationData.get('user'), pm.iterationData.get('token'), pm.iterationData.toObject().row);
            pm.environment.set('newEnv', 'v1');
            pm.collectionVariables.set('n', 5);
            pm.variables.set('obj', { a: 1 });
            pm.globals.set('g', true);
            pm.environment.unset('token');
            console.log(typeof pm.variables.get('n'), pm.environment.get('newEnv'), pm.environment.get('token'), pm.variables.get('obj').a);
            console.log(pm.variables.replaceIn('{{baseUrl}}/u/{{user}}?t={{secret.X}}&n={{nested}}&e={{$env.HOME}}&m={{missing}}'));
            console.log(/^\d+$/.test(pm.variables.replaceIn('{{$timestamp}}')), pm.environment.replaceIn('{{user}}'));
            console.log(Object.keys(pm.globals.toObject()).sort().join(','));
            "#,
        ),
        &input,
    );
    ok(&o);
    assert_eq!(
        logs(&o),
        [
            "rt ada https://env.example.com near p undefined",
            "e rt undefined true dev",
            "near o undefined",
            "rt undefined",
            "ada undefined 1",
            "number v1 env-token 1",
            "https://env.example.com/u/ada?t={{secret.X}}&n=ada-x&e={{$env.HOME}}&m={{missing}}",
            "true {{user}}",
            "g,n,nested,newEnv,obj",
        ]
    );
    assert_eq!(
        o.changes,
        vec![
            VarChange { scope: VarScope::Environment, name: "newEnv".into(), value: Some("v1".into()) },
            VarChange { scope: VarScope::CollectionVariables, name: "n".into(), value: Some("5".into()) },
            VarChange { scope: VarScope::Variables, name: "obj".into(), value: Some(r#"{"a":1}"#.into()) },
            VarChange { scope: VarScope::Globals, name: "g".into(), value: Some("true".into()) },
            VarChange { scope: VarScope::Environment, name: "token".into(), value: None },
        ]
    );
    assert_eq!(o.runtime, vars(&[("nested", "{{user}}-x"), ("newEnv", "v1"), ("n", "5"), ("obj", r#"{"a":1}"#), ("g", "true")]));
}

#[test]
fn apply_changes_in_order() {
    let mut rt = vars(&[("a", "1")]);
    apply_changes(
        &mut rt,
        &[
            VarChange { scope: VarScope::Variables, name: "b".into(), value: Some("2".into()) },
            VarChange { scope: VarScope::Environment, name: "a".into(), value: None },
            VarChange { scope: VarScope::Globals, name: "b".into(), value: Some("3".into()) },
        ],
    );
    assert_eq!(rt, vars(&[("b", "3")]));
}

// ---------------------------------------------------------------- request edits

#[test]
fn request_edits() {
    let o = run_pre(
        r#"
        pm.request.method = 'post';
        pm.request.url.query.add({ key: 'q', value: 'a b' });
        pm.request.url.query.upsert({ key: 'page', value: 2 });
        pm.request.headers.add({ key: 'X-New', value: 'n' });
        pm.request.headers.upsert({ key: 'accept', value: 'text/plain' });
        pm.request.headers.remove('X-Trace');
        pm.request.body.raw = JSON.stringify({ hello: 'world' });
        console.log(pm.request.url.toString());
        console.log(pm.request.url.getHost(), pm.request.url.getPath(), pm.request.url.protocol, pm.request.headers.get('ACCEPT'), pm.request.headers.has('x-trace'));
        console.log(pm.request.url.getPathWithQuery(), pm.request.headers.toObject()['X-New'], pm.request.body.mode, typeof pm.response);
        "#,
    );
    ok(&o);
    assert_eq!(
        logs(&o),
        ["https://api.example.com/users?page=2&q=a b", "api.example.com /users https text/plain false", "/users?page=2&q=a b n raw undefined"]
    );
    assert!(o.request_changed);
    assert_eq!(
        o.request,
        RequestView {
            method: "POST".into(),
            url: "https://api.example.com/users".into(),
            query: vec![("page".into(), "2".into()), ("q".into(), "a b".into())],
            headers: vec![("Accept".into(), "text/plain".into()), ("X-New".into(), "n".into())],
            body: Some(r#"{"hello":"world"}"#.into()),
        }
    );
}

#[test]
fn url_replacement_and_no_op() {
    let o = run_pre("pm.request.url = 'https://other.test:8443/v2/items?x=1&y=%20z#frag'; console.log(pm.request.url.port, pm.request.url.host.join('.'), pm.request.url.path.join('/'));");
    ok(&o);
    assert_eq!(logs(&o), ["8443 other.test v2/items"]);
    assert_eq!(o.request.url, "https://other.test:8443/v2/items#frag");
    assert_eq!(o.request.query, vec![("x".to_string(), "1".to_string()), ("y".to_string(), " z".to_string())]);

    let o = run_pre("pm.request.url.update('{{baseUrl}}/a'); pm.request.body.update({ mode: 'raw', raw: { x: 1 } });");
    ok(&o);
    assert_eq!(o.request.url, "{{baseUrl}}/a");
    assert!(o.request.query.is_empty());
    assert_eq!(o.request.body.as_deref(), Some(r#"{"x":1}"#));

    let o = run_pre("console.log(pm.request.method, pm.request.url.toString(), pm.request.body.raw === '')");
    ok(&o);
    assert_eq!(logs(&o), ["GET https://api.example.com/users?page=1 true"]);
    assert!(!o.request_changed);
    assert_eq!(o.request, request());
}

// ---------------------------------------------------------------- response

#[test]
fn response_api() {
    let o = run_post(
        r#"
        const j = pm.response.json();
        pm.collectionVariables.set('userId', j.id);
        console.log(pm.response.code, pm.response.status, pm.response.reason(), pm.response.responseTime, pm.response.responseSize);
        console.log(pm.response.headers.get('content-type'), pm.response.headers.has('X-RATE'), pm.cookies.get('session'), pm.cookies.has('nope'));
        console.log(pm.response.text() === JSON.stringify(j), pm.response.size().body, j.name);
        console.log(pm.info.eventName, typeof pm.request.url.toString());
        "#,
    );
    ok(&o);
    assert_eq!(
        logs(&o),
        [
            format!("200 OK OK 42.5 {}", BODY.len()).as_str(),
            "application/json; charset=utf-8 true xyz false",
            format!("true {} Ada ✓", BODY.len()).as_str(),
            "test string",
        ]
    );
    assert_eq!(o.runtime.get("userId").map(String::as_str), Some("7"));
    assert!(!o.request_changed);

    let mut input = post_input();
    input.response = ResponseInfo::new(502, "Bad Gateway", &[], b"<html>oops</html>", 1.0, 17);
    let o = ScriptEngine::new().run_post_response(&src("\npm.response.json();"), &input);
    let e = o.error.expect("json() on HTML fails");
    assert!(e.message.starts_with("JSONError:"), "{e:?}");
    assert_eq!(e.line, Some(2));
}

// ---------------------------------------------------------------- tests & assertions

#[test]
fn pm_test_results() {
    let o = run_post(
        r#"
        pm.test('sync pass', () => { pm.expect(1).to.equal(1); });
        pm.test('sync fail', () => { pm.expect(404).to.equal(200); });
        pm.test('skipped');
        pm.test.skip('explicit skip', () => {});
        pm.test('done callback', (done) => { setTimeout(() => { pm.expect(true).to.be.true; done(); }, 50); });
        pm.test('done with error', (done) => { setTimeout(() => done(new Error('nope')), 10); });
        pm.test('async fn', async () => { await null; pm.expect(2).to.equal(3); });
        pm.test('never done', (done) => {});
        pm.test('throws TypeError', () => { null.x; });
        tests['legacy ok'] = true;
        tests['legacy bad'] = 1 === 2;
        "#,
    );
    ok(&o);
    let names: Vec<&str> = o.tests.iter().map(|t| t.label.as_str()).collect();
    assert_eq!(names, ["sync pass", "sync fail", "skipped", "explicit skip", "done callback", "done with error", "async fn", "never done", "throws TypeError", "legacy ok", "legacy bad"]);
    assert!(test_named(&o, "sync pass").passed);
    let fail = test_named(&o, "sync fail");
    assert!(!fail.passed);
    assert_eq!(fail.message.as_deref(), Some("AssertionError: expected 404 to equal 200"));
    assert!(test_named(&o, "skipped").passed && test_named(&o, "skipped").skipped);
    assert!(test_named(&o, "explicit skip").skipped);
    assert!(test_named(&o, "done callback").passed, "{:?}", test_named(&o, "done callback"));
    assert_eq!(test_named(&o, "done with error").message.as_deref(), Some("Error: nope"));
    assert_eq!(test_named(&o, "async fn").message.as_deref(), Some("AssertionError: expected 2 to equal 3"));
    assert!(test_named(&o, "never done").message.as_deref().unwrap().contains("never finished"));
    assert!(test_named(&o, "throws TypeError").message.as_deref().unwrap().starts_with("TypeError: "));
    assert!(test_named(&o, "legacy ok").passed);
    assert!(!test_named(&o, "legacy bad").passed);
    assert!(o.tests.iter().all(|t| t.phase == ScriptPhase::PostResponse && t.source == "request" && t.actual.is_none()));

    // Same JSON shape as devall_http::checks::AssertionResult.
    let json = serde_json::to_value(test_named(&o, "sync fail")).unwrap();
    for key in ["label", "passed", "actual", "message"] {
        assert!(json.get(key).is_some(), "{key} missing from {json}");
    }
}

#[test]
fn chai_messages() {
    // (expression, expected failure message or None when it must pass)
    let cases: &[(&str, Option<&str>)] = &[
        ("pm.expect(1).to.equal(2)", Some("expected 1 to equal 2")),
        ("pm.expect({a: 1}).to.eql({a: 2})", Some("expected { a: 1 } to deeply equal { a: 2 }")),
        ("pm.expect({a: {b: 1}}).to.deep.equal({a: {b: 2}})", Some("expected { a: { b: 1 } } to deeply equal { a: { b: 2 } }")),
        ("pm.expect('abc').to.include('x')", Some("expected 'abc' to include 'x'")),
        ("pm.expect({a: 1}).to.have.property('b')", Some("expected { a: 1 } to have property 'b'")),
        ("pm.expect({a: 1}).to.have.property('a', 2)", Some("expected { a: 1 } to have property 'a' of 2, but got 1")),
        ("pm.expect([1, 2]).to.have.lengthOf(3)", Some("expected [ 1, 2 ] to have a length of 3 but got 2")),
        ("pm.expect([1, 2, 3]).to.have.length.above(5)", Some("expected [ 1, 2, 3 ] to have a length above 5 but got 3")),
        ("pm.expect('x').to.be.a('number')", Some("expected 'x' to be a number")),
        ("pm.expect([]).to.be.an('object')", Some("expected [] to be an object")),
        ("pm.expect(5).to.be.above(10)", Some("expected 5 to be above 10")),
        ("pm.expect(5).to.be.below(1)", Some("expected 5 to be below 1")),
        ("pm.expect(5).to.be.at.least(6)", Some("expected 5 to be at least 6")),
        ("pm.expect(5).to.be.at.most(4)", Some("expected 5 to be at most 4")),
        ("pm.expect(7).to.be.within(1, 5)", Some("expected 7 to be within 1..5")),
        ("pm.expect('abc').to.match(/^x/)", Some("expected 'abc' to match /^x/")),
        ("pm.expect(null).to.exist", Some("expected null to exist")),
        ("pm.expect(1).to.be.true", Some("expected 1 to be true")),
        ("pm.expect(true).to.be.false", Some("expected true to be false")),
        ("pm.expect(0).to.be.null", Some("expected 0 to be null")),
        ("pm.expect(1).to.be.undefined", Some("expected 1 to be undefined")),
        ("pm.expect([1]).to.be.empty", Some("expected [ 1 ] to be empty")),
        ("pm.expect(3).to.be.oneOf([1, 2])", Some("expected 3 to be one of [ 1, 2 ]")),
        ("pm.expect(1).to.not.equal(1)", Some("expected 1 to not equal 1")),
        ("pm.expect('5').to.equal(5)", Some("expected '5' to equal 5")),
        ("pm.expect(undefined).to.have.property('x')", Some("expected undefined to have property 'x'")),
        ("pm.expect({a: 1, b: 2}).to.have.keys('a')", Some("expected { a: 1, b: 2 } to have key 'a'")),
        ("pm.expect([1, 2, 3]).to.have.members([1, 2])", Some("expected [ 1, 2, 3 ] to have the same members as [ 1, 2 ]")),
        ("pm.expect({a: 1}, 'custom').to.have.property('b')", Some("custom: expected { a: 1 } to have property 'b'")),
        ("pm.expect({alpha: 1, beta: 2, gamma: 3, delta: 4}).to.be.empty", Some("expected { Object (alpha, beta, ...) } to be empty")),
        ("pm.expect(() => {}).to.throw()", Some("expected [Function] to throw an error")),
        ("pm.expect.fail('nope')", Some("nope")),
        ("pm.expect({a: {b: [1, 2]}}).to.have.nested.property('a.b[1]', 2)", None),
        ("pm.expect({a: 1, b: 2}).to.have.all.keys('a', 'b')", None),
        ("pm.expect({a: 1, b: 2}).to.have.any.keys('c', 'a')", None),
        ("pm.expect({a: 1, b: 2}).to.include.keys('a')", None),
        ("pm.expect([1, 2, 3]).to.include.members([1, 2])", None),
        ("pm.expect([{a: 1}]).to.deep.include({a: 1})", None),
        ("pm.expect({a: 1, b: 2}).to.include({a: 1})", None),
        ("pm.expect('').to.be.empty; pm.expect({}).to.be.empty; pm.expect([]).to.be.an('array').that.is.empty", None),
        ("pm.expect(1.5).to.be.closeTo(1.4, 0.2)", None),
        ("pm.expect(() => { throw new TypeError('bad thing') }).to.throw(TypeError, 'bad')", None),
        ("pm.expect(7).to.be.within(1, 10)", None),
        ("pm.expect([1, 2, 3]).to.have.length.above(2)", None),
        ("pm.expect({a: 1}).to.have.property('a').that.is.a('number')", None),
        ("pm.expect('hello').to.be.a('string').and.have.lengthOf(5)", None),
        ("pm.expect(new Date()).to.be.a('date')", None),
        ("pm.expect(null).to.not.exist", None),
        ("pm.expect(NaN).to.be.NaN", None),
        ("pm.expect(200).to.be.oneOf([200, 201])", None),
        ("pm.expect({x: [1, {y: 2}]}).to.eql({x: [1, {y: 2}]})", None),
        ("pm.expect(5).to.satisfy(n => n > 3)", None),
        ("pm.expect('abc').to.have.string('b')", None),
        ("pm.expect(new Map([['a', 1]])).to.have.keys('a')", None),
        ("pm.expect(new Set([1])).to.include(1)", None),
        ("pm.expect('x').to.be.ok; pm.expect(0).to.not.be.ok", None),
        ("pm.expect([3, 1]).to.not.include(2)", None),
        ("pm.expect({a: 1}).to.not.have.property('b')", None),
        ("expect(1).to.equal(1)", None),
    ];
    let code: String = cases.iter().enumerate().map(|(i, (expr, _))| format!("pm.test('t{i}', () => {{ {expr}; }});\n")).collect();
    let o = run_post(&code);
    ok(&o);
    assert_eq!(o.tests.len(), cases.len());
    for (i, (expr, expected)) in cases.iter().enumerate() {
        let t = &o.tests[i];
        match expected {
            None => assert!(t.passed, "{expr} should pass, got {:?}", t.message),
            Some(msg) => {
                assert!(!t.passed, "{expr} should fail");
                assert_eq!(t.message.as_deref(), Some(format!("AssertionError: {msg}").as_str()), "{expr}");
            }
        }
    }
}

#[test]
fn response_assertions() {
    let cases: &[(&str, Option<&str>)] = &[
        ("pm.response.to.have.status(200)", None),
        ("pm.response.to.have.status('OK')", None),
        ("pm.response.to.have.status(201)", Some("expected response to have status code 201 but got 200")),
        ("pm.response.to.not.have.status(200)", Some("expected response to not have status code 200")),
        ("pm.response.to.have.header('content-type')", None),
        ("pm.response.to.have.header('X-Missing')", Some("expected response to have header with key 'X-Missing'")),
        ("pm.response.to.have.header('X-Rate', '11')", Some("expected 'X-Rate' response header to be '11' but got '10'")),
        ("pm.response.to.have.header('X-Rate', 10)", None),
        ("pm.response.to.be.ok", None),
        ("pm.response.to.be.json", None),
        ("pm.response.to.be.success", None),
        ("pm.response.to.be.withBody", None),
        ("pm.response.to.not.be.error", None),
        ("pm.response.to.be.notFound", Some("expected response code to be 404 but found 200")),
        ("pm.expect(pm.response).to.have.status(200)", None),
        ("pm.expect(pm.response).to.be.ok", None),
        ("pm.response.to.have.jsonBody('nested.token')", None),
        ("pm.response.to.have.jsonBody('nested.list[1].v', 2)", None),
        ("pm.response.to.have.jsonBody('nested.nope')", Some("expected response body json to have property 'nested.nope'")),
        ("pm.response.to.have.jsonSchema({ type: 'object', required: ['id', 'name'], properties: { id: { type: 'integer' }, name: { type: 'string' }, tags: { type: 'array', items: { type: 'string' } } } })", None),
        ("pm.response.to.have.jsonSchema({ type: 'object', properties: { id: { type: 'string' } } })", Some("expected data to satisfy schema but found following errors: \ndata.id should be string but is number")),
        ("pm.response.to.have.body('nope')", Some(&*format!("expected response body to equal 'nope' but got {}", "'{\"id\":7,\"name\":\"Ada ✓\",\"tags\":[\"a\",\"b\"],\"nested\":{\"token\":\"t0k3n\",\"list\":[{\"v\":1},{\"v\":2}]}}'").leak())),
        ("pm.response.to.have.body(/t0k3n/)", None),
        ("pm.expect(pm.response.code).to.eql(200)", None),
        ("pm.expect(pm.response.responseTime).to.be.below(1000)", None),
        ("pm.expect(pm.response.headers.get('Content-Type')).to.include('application/json')", None),
        ("pm.expect(pm.response.json().tags).to.have.members(['b', 'a'])", None),
        ("pm.expect(tv4.validate(pm.response.json(), { type: 'object', required: ['missing'] })).to.be.false", None),
        ("pm.expect(1).to.be.success", Some(".success can only be used on pm.response (got 1)")),
    ];
    let code: String = cases.iter().enumerate().map(|(i, (expr, _))| format!("pm.test('t{i}', () => {{ {expr}; }});\n")).collect();
    let o = run_post(&code);
    ok(&o);
    for (i, (expr, expected)) in cases.iter().enumerate() {
        let t = &o.tests[i];
        match expected {
            None => assert!(t.passed, "{expr} should pass, got {:?}", t.message),
            Some(msg) => {
                assert!(!t.passed, "{expr} should fail");
                assert_eq!(t.message.as_deref(), Some(format!("AssertionError: {msg}").as_str()), "{expr}");
            }
        }
    }
}

// ---------------------------------------------------------------- limits

fn fast_engine() -> ScriptEngine {
    ScriptEngine::new().with_timeout(Duration::from_millis(200))
}

#[test]
fn timeouts_stop_infinite_loops() {
    for code in [
        "pm.variables.set('before', 1);\nwhile (true) {}",
        "pm.variables.set('before', 1);\ntry { while (true) {} } catch (e) { pm.variables.set('caught', 1); }",
        "pm.variables.set('before', 1);\nasync function spin() { for (;;) { await null; } }\nspin();",
        "pm.variables.set('before', 1);\npm.test('loops', () => { for (;;) {} });",
        "pm.variables.set('before', 1);\nsetTimeout(function again() { setTimeout(again, 1); for (let i = 0; i < 1e5; i++); }, 1);",
    ] {
        let start = Instant::now();
        let o = fast_engine().run_pre_request(&src(code), &pre_input());
        let e = o.error.as_ref().unwrap_or_else(|| panic!("{code} should time out"));
        assert_eq!(e.kind, ScriptErrorKind::Timeout, "{code}: {e:?}");
        assert_eq!(e.message, "timed out after 200 ms");
        assert!(start.elapsed() < Duration::from_secs(5), "{code} took {:?}", start.elapsed());
        // Changes made before the timeout are kept; nothing after it ran.
        assert_eq!(o.changes.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["before"], "{code}");
    }
}

#[test]
fn never_settling_await_and_runaway_intervals() {
    let o = fast_engine().run_pre_request(&src("await new Promise(() => {});"), &pre_input());
    let e = o.error.unwrap();
    assert_eq!(e.kind, ScriptErrorKind::Timeout);
    assert!(e.message.contains("never finished"), "{e:?}");

    let o = run_pre("let n = 0; setInterval(() => { n++; }, 10); pm.variables.set('ok', 1);");
    ok(&o);
    assert!(o.logs.iter().any(|l| l.level == LogLevel::Warn && l.message.contains("10000 callbacks")), "{:?}", o.logs);
}

#[test]
fn memory_limit() {
    let engine = ScriptEngine::new().with_memory_limit(16 * 1024 * 1024);
    let o = engine.run_pre_request(&src("pm.variables.set('before', 1);\nconst keep = [];\nwhile (true) keep.push(new Array(100000).fill(Math.random()));"), &pre_input());
    let e = o.error.expect("allocation must fail");
    assert_eq!(e.kind, ScriptErrorKind::Memory, "{e:?}");
    assert_eq!(e.message, "ran out of memory (limit 16 MiB)");
    assert_eq!(o.changes.len(), 1, "results are still collected after running out of memory");

    // A fresh run in the same engine is unaffected.
    let o = engine.run_pre_request(&src("pm.variables.set('x', 'y')"), &pre_input());
    ok(&o);
}

#[test]
fn stack_overflow_is_a_range_error() {
    let o = run_pre("function f() { return f() + 1; }\nf();");
    let e = o.error.unwrap();
    assert_eq!(e.kind, ScriptErrorKind::Runtime);
    assert_eq!(e.message, "RangeError: Maximum call stack size exceeded");
    assert_eq!(e.line, Some(1));
}

// ---------------------------------------------------------------- errors

#[test]
fn errors_report_line_numbers() {
    let cases: &[(&str, ScriptErrorKind, &str, Option<u32>)] = &[
        ("let a = 1;\n\nfoo();", ScriptErrorKind::Runtime, "ReferenceError: foo is not defined", Some(3)),
        ("let a = 1;\nlet b = ;", ScriptErrorKind::Syntax, "SyntaxError: unexpected token in expression: ';'", Some(2)),
        ("\n\n  pm.expect(1).to.equal(2);", ScriptErrorKind::Runtime, "AssertionError: expected 1 to equal 2", Some(3)),
        ("throw 'boom'", ScriptErrorKind::Runtime, "Uncaught boom", None),
        ("throw { code: 1 }", ScriptErrorKind::Runtime, "Uncaught {\"code\":1}", None),
        ("setTimeout(() => {\n  undefinedFn();\n}, 5);", ScriptErrorKind::Runtime, "ReferenceError: undefinedFn is not defined", Some(2)),
        ("await null;\nnull.x;", ScriptErrorKind::Runtime, "TypeError: cannot read property 'x' of null", Some(2)),
        ("pm.sendRequest('https://example.com', () => {});", ScriptErrorKind::Runtime, "Error: pm.sendRequest is not supported in /dev/all scripts — chain requests with captures or a collection run instead", Some(1)),
    ];
    for (code, kind, message, line) in cases {
        let o = run_pre(code);
        let e = o.error.as_ref().unwrap_or_else(|| panic!("{code} should fail"));
        assert_eq!(e.kind, *kind, "{code}: {e:?}");
        assert_eq!(e.message, *message, "{code}");
        assert_eq!(e.line, *line, "{code}: {e:?}");
        assert_eq!(e.source, "request");
        assert_eq!(e.phase, ScriptPhase::PreRequest);
    }
    let e = run_pre("let a = 1;\n\nfoo();").error.unwrap();
    assert_eq!(e.to_string(), "pre-request script (request) line 3:1: ReferenceError: foo is not defined");
    assert_eq!(serde_json::to_value(&e).unwrap()["kind"], "runtime");
}

#[test]
fn syntax_check() {
    let engine = ScriptEngine::new();
    assert_eq!(engine.check_syntax("pm.test('x', () => { pm.expect(1).to.eql(1); });"), None);
    assert_eq!(engine.check_syntax("const r = await Promise.resolve(1);\nconsole.log(r);"), None);
    assert_eq!(engine.check_syntax(""), None);
    // Never executes anything.
    assert_eq!(engine.check_syntax("while (true) {}"), None);
    let e = engine.check_syntax("ok();\nlet x = ;\n").unwrap();
    assert_eq!((e.kind, e.line), (ScriptErrorKind::Syntax, Some(2)));
    // (`let = 5` is valid sloppy-mode JavaScript; `const = 5` is not.)
    let e = engine.check_syntax("const = 5").unwrap();
    assert_eq!(e.kind, ScriptErrorKind::Syntax);
    assert_eq!(e.line, Some(1));
    assert!(e.column.unwrap() <= 8, "column is relative to the user's code: {e:?}");
    let e = engine.check_syntax("if (x) {").unwrap();
    assert_eq!(e.kind, ScriptErrorKind::Syntax);
}

// ---------------------------------------------------------------- chains

#[test]
fn chain_order_and_shared_state() {
    let scripts = [
        ScriptSource::new("collection", "pm.variables.set('order', 'c'); pm.request.headers.add({ key: 'X-Coll', value: '1' });"),
        ScriptSource::new("folder api", "pm.variables.set('order', pm.variables.get('order') + 'f'); console.log(pm.request.headers.get('X-Coll'));"),
        ScriptSource::new("blank", "   \n"),
        ScriptSource::new("request", "pm.variables.set('order', pm.variables.get('order') + 'r'); pm.test('from request', () => {});"),
    ];
    let o = ScriptEngine::new().run_pre_request(&scripts, &pre_input());
    ok(&o);
    assert_eq!(o.scripts_run, 3);
    assert_eq!(o.runtime.get("order").map(String::as_str), Some("cfr"));
    assert_eq!(o.logs.len(), 1);
    assert_eq!((o.logs[0].source.as_str(), o.logs[0].message.as_str()), ("folder api", "1"));
    assert!(o.request.header("x-coll").is_some());
    assert_eq!(o.tests[0].source, "request");
}

#[test]
fn chain_stops_at_first_error() {
    let scripts = [
        ScriptSource::new("collection", "pm.variables.set('a', '1'); pm.request.method = 'PUT';"),
        ScriptSource::new("folder api", "pm.variables.set('b', '2');\nboom();"),
        ScriptSource::new("request", "pm.variables.set('c', '3');"),
    ];
    let o = ScriptEngine::new().run_pre_request(&scripts, &pre_input());
    let e = o.error.as_ref().unwrap();
    assert_eq!((e.source.as_str(), e.line), ("folder api", Some(2)));
    assert_eq!(o.scripts_run, 2);
    assert_eq!(o.runtime, vars(&[("a", "1"), ("b", "2")]));
    assert_eq!(o.request, request(), "a failed chain doesn't edit the request");
    assert!(!o.request_changed);
}

// ---------------------------------------------------------------- crypto & encoding

#[test]
fn crypto_js_known_vectors() {
    let o = run_pre(
        r#"
        const C = require('crypto-js');
        console.log(C === CryptoJS);
        console.log(CryptoJS.HmacSHA256('what do ya want for nothing?', 'Jefe').toString());
        console.log(CryptoJS.HmacSHA256('what do ya want for nothing?', 'Jefe').toString(CryptoJS.enc.Base64));
        console.log(CryptoJS.HmacSHA1('what do ya want for nothing?', 'Jefe').toString(CryptoJS.enc.Hex));
        console.log(CryptoJS.HmacMD5('what do ya want for nothing?', 'Jefe') + '');
        console.log(CryptoJS.HmacSHA512('what do ya want for nothing?', 'Jefe').toString());
        console.log(CryptoJS.SHA256('abc').toString());
        console.log(CryptoJS.SHA1('abc').toString(), CryptoJS.MD5('abc').toString(), CryptoJS.MD5('').toString());
        console.log(CryptoJS.SHA512('abc').toString().slice(0, 32), CryptoJS.SHA224('abc').toString().slice(0, 16), CryptoJS.SHA384('abc').toString().slice(0, 16));
        console.log(CryptoJS.enc.Base64.stringify(CryptoJS.enc.Utf8.parse('hello')), CryptoJS.enc.Utf8.stringify(CryptoJS.enc.Base64.parse('aGVsbG8=')));
        console.log(CryptoJS.enc.Hex.parse('48656c6c6f').toString(CryptoJS.enc.Utf8), CryptoJS.enc.Latin1.parse('\xff').toString());
        const wa = CryptoJS.SHA256('abc');
        console.log(wa.sigBytes, wa.words.length, wa.words[0]);
        console.log(CryptoJS.algo.SHA256.create().update('a').update('bc').finalize().toString());
        console.log(CryptoJS.algo.HMAC.create(CryptoJS.algo.SHA256, 'Jefe').update('what do ya want ').update('for nothing?').finalize().toString());
        console.log(CryptoJS.HmacSHA256(CryptoJS.enc.Utf8.parse('what do ya want for nothing?'), CryptoJS.enc.Hex.parse('4a656665')).toString());
        console.log(CryptoJS.SHA256('héllo').toString());
        console.log(CryptoJS.lib.WordArray.random(16).sigBytes, CryptoJS.lib.WordArray.create([0x01020304, 0x05060708], 6).toString());
        let e1 = ''; try { CryptoJS.AES.encrypt('a', 'b'); } catch (e) { e1 = e.message; }
        console.log(e1);
        "#,
    );
    ok(&o);
    let sha_utf8 = hex::encode(sha2::Sha256::digest("héllo".as_bytes()));
    let hmac_hex = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
    let hmac_b64 = base64::engine::general_purpose::STANDARD.encode(hex::decode(hmac_hex).unwrap());
    assert_eq!(
        logs(&o),
        [
            "true",
            hmac_hex,
            hmac_b64.as_str(),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79",
            "750c783e6ab0b503eaa86e310a5db738",
            "164b7a7bfcf819e2e395fbe73b56e0a387bd64222e831fd610270cd7ea2505549758bf75c05a994a6d034f65f8f0e6fdcaeab1a34d4a6b4b636e070a38bce737",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "a9993e364706816aba3e25717850c26c9cd0d89d 900150983cd24fb0d6963f7d28e17f72 d41d8cd98f00b204e9800998ecf8427e",
            "ddaf35a193617abacc417349ae204131 23097d223405d822 cb00753f45a35e8b",
            "aGVsbG8= hello",
            "Hello ff",
            "32 8 -1166534977",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            hmac_hex,
            hmac_hex,
            sha_utf8.as_str(),
            "16 010203040506",
            "CryptoJS.AES is not supported in /dev/all scripts — available: MD5, SHA1, SHA224, SHA256, SHA384, SHA512 and their Hmac* variants",
        ]
    );
}

#[test]
fn encoding_helpers() {
    let o = run_pre(
        r#"
        console.log(btoa('hello'), atob('aGVsbG8='), atob('aGk'));
        let e2 = ''; try { btoa('✓'); } catch (e) { e2 = e.name; }
        console.log(e2);
        console.log(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(crypto.randomUUID()), crypto.randomUUID() !== crypto.randomUUID());
        console.log(Buffer.from('hi').toString('base64'), Buffer.from('aGk=', 'base64').toString(), Buffer.from('hi').toString('hex'), Buffer.from('ü').length);
        console.log(new TextDecoder().decode(new TextEncoder().encode('ü✓')), crypto.getRandomValues(new Uint8Array(8)).length);
        console.log(require('uuid').v4().length, require('btoa')('a'), typeof require('lodash').get);
        "#,
    );
    ok(&o);
    assert_eq!(logs(&o), ["aGVsbG8= hello hi", "InvalidCharacterError", "true true", "aGk= hi 6869 2", "ü✓ 8", "36 YQ== function"]);
}

// ---------------------------------------------------------------- compatibility shims

#[test]
fn bruno_pre_request_shims() {
    let o = run_pre(
        r#"
        bru.setEnvVar('host', 'h1'); bru.setVar('rv', 'x');
        console.log(bru.getVar('rv'), bru.getEnvVar('host'), bru.getEnvName(), bru.interpolate('{{rv}}!'));
        req.setHeader('X-Bru', 'yes'); req.setMethod('patch'); req.setUrl('https://b.test/p?z=9'); req.setBody({ k: 1 });
        console.log(req.getUrl(), req.getMethod(), req.getHeader('x-bru'), req.getBody().k, req.getName());
        console.log(bru.getProcessEnv('HOME'));
        "#,
    );
    ok(&o);
    assert_eq!(logs(&o)[..2], ["x h1 dev x!", "https://b.test/p?z=9 PATCH yes 1 Get user"]);
    assert!(o.logs[2].message.contains("process environment is not available"), "{:?}", o.logs);
    assert_eq!(o.logs[3].message, "undefined");
    assert_eq!(o.changes[0], VarChange { scope: VarScope::Environment, name: "host".into(), value: Some("h1".into()) });
    assert_eq!(o.changes[1].scope, VarScope::Variables);
    assert_eq!(o.request.method, "PATCH");
    assert_eq!(o.request.url, "https://b.test/p");
    assert_eq!(o.request.query, vec![("z".to_string(), "9".to_string())]);
    assert_eq!(o.request.header("X-Bru"), Some("yes"));
    assert_eq!(o.request.body.as_deref(), Some(r#"{"k":1}"#));
}

#[test]
fn bruno_post_response_shims() {
    let o = run_post(
        r#"
        test('status is 200', () => expect(res.getStatus()).to.equal(200));
        test('name', () => expect(res.body.name).to.equal('nope'));
        console.log(res.getHeader('content-type'), res.getBody().name, res('nested.token'), res('nested.list[1].v'), res.body.tags.length, res.status, res.headers['x-rate'], res.getResponseTime());
        bru.setVar('tok', res('nested.token'));
        "#,
    );
    ok(&o);
    assert_eq!(logs(&o), ["application/json; charset=utf-8 Ada ✓ t0k3n 2 2 200 10 42.5"]);
    assert!(test_named(&o, "status is 200").passed);
    assert_eq!(test_named(&o, "name").message.as_deref(), Some("AssertionError: expected 'Ada ✓' to equal 'nope'"));
    assert_eq!(o.runtime.get("tok").map(String::as_str), Some("t0k3n"));
}

#[test]
fn legacy_postman_globals() {
    let o = run_post(
        r#"
        postman.setEnvironmentVariable('legacy', 'v');
        postman.setGlobalVariable('lg', 1);
        console.log(postman.getEnvironmentVariable('legacy'), responseCode.code, JSON.parse(responseBody).id, responseHeaders['X-Rate'], typeof responseTime, postman.getResponseHeader('x-rate'), iteration);
        tests['legacy status'] = responseCode.code === 200;
        postman.setNextRequest('Other');
        "#,
    );
    ok(&o);
    assert_eq!(logs(&o)[0], "v 200 7 10 number 10 0");
    assert!(o.logs[1].message.contains("setNextRequest is not supported"));
    assert!(test_named(&o, "legacy status").passed);
    assert_eq!(o.changes.iter().map(|c| c.scope).collect::<Vec<_>>(), [VarScope::Environment, VarScope::Globals]);
}

#[test]
fn lodash_subset() {
    let o = run_pre(
        r#"
        console.log(_.get({ a: { b: [1, { c: 'x' }] } }, 'a.b[1].c'), _.isEqual({ a: [1] }, { a: [1] }), _.isEmpty({}), _.uniq([1, 1, 2]).length, _.pick({ a: 1, b: 2 }, 'a').a, _.find([{ id: 1 }, { id: 2 }], { id: 2 }).id, _.random(5, 5));
        let m = ''; try { _.zipWith(); } catch (e) { m = e.message; }
        console.log(m);
        "#,
    );
    ok(&o);
    assert_eq!(logs(&o)[0], "x true true 2 1 2 5");
    assert!(logs(&o)[1].contains("_.zipWith is not available"));
}

#[test]
fn timers_run_in_virtual_time_and_top_level_await() {
    let start = Instant::now();
    let o = run_pre(
        r#"
        setTimeout(() => console.log('c'), 20);
        setTimeout(() => console.log('b'), 10);
        setImmediate(() => console.log('a'));
        const id = setTimeout(() => console.log('never'), 5); clearTimeout(id);
        await new Promise((r) => setTimeout(r, 60000));
        console.log('after await');
        let n = 0; const iv = setInterval(() => { if (++n === 3) { clearInterval(iv); console.log('interval done'); } }, 1);
        await bru.sleep(1000);
        pm.variables.set('done', n);
        "#,
    );
    ok(&o);
    assert_eq!(logs(&o), ["a", "b", "c", "after await", "interval done"]);
    assert_eq!(o.runtime.get("done").map(String::as_str), Some("3"));
    assert!(start.elapsed() < Duration::from_secs(5), "virtual time doesn't wait");
}

#[test]
fn sandbox_has_no_host_access() {
    let o = run_pre(
        r#"
        console.log(typeof process, typeof std, typeof os, typeof fetch, typeof XMLHttpRequest, typeof __devall_native, typeof __devall_input, typeof require, typeof Deno, typeof module);
        let threw = ''; try { require('fs'); } catch (e) { threw = e.message; }
        console.log(threw);
        const data = 1; let request = 2; var CryptoJS2 = 3;
        console.log(data + request + CryptoJS2);
        console.log(pm.info.eventName, pm.info.requestName, pm.info.iteration, pm.info.iterationCount, pm.info.requestId, pm.execution.location.join('/'));
        "#,
    );
    ok(&o);
    assert_eq!(logs(&o)[0], "undefined undefined undefined undefined undefined undefined undefined function undefined undefined");
    assert!(logs(&o)[1].starts_with("require('fs') is not available in /dev/all scripts"), "{}", logs(&o)[1]);
    assert_eq!(logs(&o)[2], "6");
    assert_eq!(logs(&o)[3], "prerequest Get user 0 1 01ABC devall/users/get-user.req.yaml");
    // Every run starts from a clean context.
    let o = run_pre("globalThis.leak = 1; Object.prototype.polluted = true;");
    ok(&o);
    let o = run_pre("console.log(typeof leak, ({}).polluted)");
    assert_eq!(logs(&o), ["undefined undefined"]);
}

#[test]
fn skip_request() {
    let o = run_pre("pm.execution.skipRequest();");
    ok(&o);
    assert!(o.skip_request);
    let o = run_post("pm.execution.skipRequest();");
    ok(&o);
    assert!(!o.skip_request);
    assert!(o.logs[0].message.contains("only works in pre-request"));
}

// ---------------------------------------------------------------- integration helpers

struct Secrets;
impl SecretLookup for Secrets {
    fn lookup(&self, name: &str) -> Option<String> {
        (name == "API_KEY").then(|| "sk-very-secret".to_string())
    }
}

#[test]
fn finds_literal_secret_names() {
    assert_eq!(secret_names("pm.secrets.get('A'); await pm.vault.get(\"B\"); pm.secrets.get(name); pm.secrets.get(`C`); pm.vault.get( 'A' ); pm.secrets.get(`x${y}`)"), ["A", "C", "B"]);
}

#[test]
fn secrets_are_available_and_masked() {
    let mut req = HttpRequestFile::new("Sign");
    req.url = "https://api.test/x".into();
    let scripts = src(
        "const k = pm.secrets.get('API_KEY');\nconsole.log('key is', k, typeof pm.secrets.get('OTHER'), pm.variables.get('secret.API_KEY'));\npm.request.headers.add({ key: 'X-Sig', value: CryptoJS.HmacSHA256('msg', k).toString() });\nthrow new Error('leak ' + k);",
    );
    let env = ScriptEnv { secrets: Some(&Secrets), ..Default::default() };
    let mut runtime = Variables::new();
    let out = apply_pre_request(&ScriptEngine::new(), &scripts, &mut req, &mut runtime, &env);
    assert_eq!(out.report.logs[0].message, "key is •••••• undefined undefined");
    assert_eq!(out.error.as_ref().unwrap().message, "Error: leak ••••••");
    assert!(out.secret_values.contains("sk-very-secret"));

    let scripts = src("pm.request.headers.add({ key: 'X-Sig', value: CryptoJS.HmacSHA256('msg', pm.secrets.get('API_KEY')).toString() });");
    let out = apply_pre_request(&ScriptEngine::new(), &scripts, &mut req, &mut runtime, &env);
    assert!(out.should_send());
    let expected = hex::encode(crate::natives::hmac("sha256", b"sk-very-secret", b"msg").unwrap());
    assert_eq!(req.headers.iter().find(|h| h.name == "X-Sig").map(|h| h.value.as_str()), Some(expected.as_str()));

    // Without a lookup, nothing is exposed.
    let out = apply_pre_request(&ScriptEngine::new(), &src("console.log(typeof pm.secrets.get('API_KEY'))"), &mut req, &mut runtime, &ScriptEnv::default());
    assert_eq!(out.report.logs[0].message, "undefined");
}

fn disabled(name: &str, value: &str) -> KeyValue {
    KeyValue { name: name.into(), value: value.into(), enabled: false, description: None }
}

#[test]
fn apply_pre_request_edits_the_request_file() {
    let mut req = HttpRequestFile::new("R");
    req.url = "{{baseUrl}}/items?fixed=1".into();
    req.params = vec![KeyValue::new("page", "1"), disabled("off", "x")];
    req.headers = vec![KeyValue::new("Accept", "*/*"), disabled("X-Off", "1")];
    let env_vars = vars(&[("baseUrl", "https://e.test")]);
    let env = ScriptEnv { environment: Some(&env_vars), environment_name: Some("dev"), ..Default::default() };
    let mut runtime = vars(&[("keep", "1")]);

    // A script that only reads leaves the request untouched.
    let before = req.clone();
    let out = apply_pre_request(&ScriptEngine::new(), &src("console.log(pm.request.url.toString())"), &mut req, &mut runtime, &env);
    assert_eq!(out.report.logs[0].message, "{{baseUrl}}/items?fixed=1&page=1");
    assert!(!out.request_changed);
    assert_eq!(req, before);

    let code = r#"
        pm.request.headers.upsert({ key: 'X-Req', value: pm.variables.replaceIn('{{baseUrl}}') });
        pm.request.url.query.remove('fixed');
        pm.request.body.raw = '{"a":1}';
        pm.environment.set('seen', '1');
    "#;
    let out = apply_pre_request(&ScriptEngine::new(), &src(code), &mut req, &mut runtime, &env);
    assert!(out.should_send() && out.request_changed, "{out:?}");
    assert_eq!(req.url, "{{baseUrl}}/items");
    assert_eq!(req.params, vec![KeyValue::new("page", "1"), disabled("off", "x")]);
    assert_eq!(req.headers, vec![KeyValue::new("Accept", "*/*"), KeyValue::new("X-Req", "https://e.test"), disabled("X-Off", "1")]);
    assert_eq!(req.body, Body::Json { content: r#"{"a":1}"#.into() });
    assert_eq!(runtime, vars(&[("keep", "1"), ("seen", "1")]));

    // Text bodies keep their content type; failures leave the request alone but keep variables.
    req.body = Body::Text { content: "old".into(), content_type: Some("text/csv".into()) };
    let out = apply_pre_request(&ScriptEngine::new(), &src("pm.request.body.raw = 'a,b'; pm.request.method = 'PUT';"), &mut req, &mut runtime, &env);
    assert!(out.request_changed);
    assert_eq!(req.body, Body::Text { content: "a,b".into(), content_type: Some("text/csv".into()) });
    assert_eq!(req.method, "PUT");

    let out = apply_pre_request(&ScriptEngine::new(), &src("pm.request.method = 'DELETE'; pm.variables.set('x', '1'); boom();"), &mut req, &mut runtime, &env);
    assert!(!out.should_send());
    assert_eq!(out.report.errors.len(), 1);
    assert_eq!(req.method, "PUT");
    assert_eq!(runtime.get("x").map(String::as_str), Some("1"));

    let out = apply_pre_request(&ScriptEngine::new(), &src("pm.execution.skipRequest()"), &mut req, &mut runtime, &env);
    assert!(out.skip_request && !out.should_send() && out.report.skipped);

    // Nothing to run: no work, no report.
    let out = apply_pre_request(&ScriptEngine::new(), &[], &mut req, &mut runtime, &env);
    assert_eq!(out, PreOutcome::default());
}

#[test]
fn apply_post_response_collects_tests_and_variables() {
    let req = HttpRequestFile::new("Login");
    let r = response();
    let resp = ResponseInfo::new(r.code, &r.status, &r.headers, BODY.as_bytes(), 12.0, BODY.len());
    let mut runtime = Variables::new();
    let data = vars(&[("row", "2")]);
    let env = ScriptEnv { iteration_data: Some(&data), iteration: 1, iteration_count: 3, ..Default::default() };
    let code = "pm.test('ok', () => pm.response.to.have.status(200)); pm.test('bad', () => pm.expect(pm.response.code).to.equal(500)); pm.environment.set('token', pm.response.json().nested.token); console.log(pm.info.iteration, pm.info.iterationCount, pm.iterationData.get('row'));";
    let out = apply_post_response(&ScriptEngine::new(), &src(code), &req, &resp, &mut runtime, &env);
    assert!(out.error.is_none(), "{out:?}");
    assert!(out.failed());
    assert_eq!(out.tests.len(), 2);
    assert_eq!(out.report.logs[0].message, "1 3 2");
    assert_eq!(runtime.get("token").map(String::as_str), Some("t0k3n"));
    assert_eq!(out.report.scripts_run, 1);
}

#[test]
fn collects_folder_scripts_outermost_first() {
    let dir = tempfile::tempdir().unwrap();
    let p = Project::init(dir.path(), "Scripts").unwrap();
    let scripts = |pre: &str, post: &str| Some(Scripts { pre_request: pre.into(), post_response: post.into() });
    p.save_folder("", &FolderFile { scripts: scripts("// root pre", ""), ..Default::default() }).unwrap();
    let api = p.create_folder("", "api").unwrap();
    let users = p.create_folder(&api, "users").unwrap();
    p.save_folder(&api, &FolderFile { scripts: scripts("// api pre", "// api post"), ..Default::default() }).unwrap();
    let mut req = HttpRequestFile::new("Get");
    req.scripts = scripts("// req pre", "// req post");
    let rel = p.create_request(&users, req.clone()).unwrap();

    let chain = collect_scripts(&p, &rel, &req).unwrap();
    let names = |v: &[ScriptSource]| v.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&chain.pre), ["collection", "folder api", "request"]);
    assert_eq!(names(&chain.post), ["folder api", "request"]);
    assert_eq!(chain.pre[1].code, "// api pre");

    // Unsaved drafts inherit only the root folder.
    let chain = collect_scripts(&p, "", &HttpRequestFile::new("Draft")).unwrap();
    assert_eq!(names(&chain.pre), ["collection"]);
    assert!(chain.post.is_empty());
}

#[test]
fn request_view_from_file() {
    let mut req = HttpRequestFile::new("V");
    req.method = " post ".into();
    req.url = "https://x.test/a?b=1%202&c#top".into();
    req.params = vec![KeyValue::new("d", "{{v}}"), disabled("e", "1")];
    req.headers = vec![KeyValue::new(" H ", "v"), disabled("Off", "1"), KeyValue::new("", "ignored")];
    req.body = Body::Xml { content: "<a/>".into() };
    let v = RequestView::from(&req);
    assert_eq!(v.method, "POST");
    assert_eq!(v.url, "https://x.test/a#top");
    assert_eq!(v.query, vec![("b".into(), "1 2".into()), ("c".into(), String::new()), ("d".into(), "{{v}}".into())]);
    assert_eq!(v.headers, vec![("H".to_string(), "v".to_string())]);
    assert_eq!(v.body.as_deref(), Some("<a/>"));
    assert_eq!(v.full_url(), "https://x.test/a?b=1 2&c=&d={{v}}#top");
    req.body = Body::Form { fields: vec![KeyValue::new("a", "1")] };
    assert_eq!(RequestView::from(&req).body, None);
}
