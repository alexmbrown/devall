// Desktop loopback sign-in. The app opens this page with ?port=<loopback port>&state=<random>;
// after sign-in the Firebase refresh token is POSTed to http://127.0.0.1:<port>/callback.
// The target is always the loopback address (never a URL taken from the query) and the app only
// accepts the callback with its own `state`. A form POST keeps the token out of browser history.
import { auth, mountSignIn, signOut } from "./auth.js";

const params = new URLSearchParams(location.search);
const port = Number(params.get("port"));
const state = params.get("state") ?? "";
const valid = Number.isInteger(port) && port >= 1024 && port <= 65535 && /^[A-Za-z0-9_-]{16,128}$/.test(state);

function handOff(fields) {
  const form = document.createElement("form");
  form.method = "POST";
  form.action = `http://127.0.0.1:${port}/callback`;
  for (const [name, value] of Object.entries(fields)) {
    const input = document.createElement("input");
    input.type = "hidden";
    input.name = name;
    input.value = value;
    form.append(input);
  }
  document.body.append(form);
  form.submit();
}

if (!valid) {
  document.getElementById("bad").hidden = false;
} else {
  await mountSignIn(document.getElementById("signin"), async (user) => {
    document.getElementById("signin").hidden = true;
    document.getElementById("done").hidden = false;
    const fields = { state, refresh_token: user.refreshToken, uid: user.uid, email: user.email ?? "" };
    // Signing this tab out doesn't revoke the refresh token the app keeps in the OS keychain.
    await signOut(auth);
    handOff(fields);
  });
}
