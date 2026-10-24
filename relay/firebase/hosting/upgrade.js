// Upgrade: sign in with the same account as the app, then continue to the Stripe Payment Link
// with client_reference_id=<uid> so the stripeWebhook function can attach the subscription.
// Supporters can also reserve subdomains here (claimSubdomain / releaseSubdomain callables).
import { app, mountSignIn } from "./auth.js";
import { stripeCustomerPortal, stripePaymentLink } from "./config.js";
import { getFunctions, httpsCallable } from "https://www.gstatic.com/firebasejs/13.1.0/firebase-functions.js";
import { getFirestore, doc, getDoc } from "https://www.gstatic.com/firebasejs/13.1.0/firebase-firestore-lite.js";

const functions = getFunctions(app, "us-central1");
const claim = httpsCallable(functions, "claimSubdomain");
const release = httpsCallable(functions, "releaseSubdomain");

function renderSubdomains(list) {
  const ul = document.getElementById("subs");
  ul.replaceChildren();
  for (const name of list) {
    const li = document.createElement("li");
    const code = document.createElement("code");
    code.textContent = name;
    const btn = document.createElement("button");
    btn.textContent = "Release";
    btn.className = "inline";
    btn.addEventListener("click", async () => {
      if (!confirm(`Release ${name}? Someone else could claim it.`)) return;
      try {
        const r = await release({ name });
        renderSubdomains(r.data.subdomains);
      } catch (e) {
        setMsg(e.message, true);
      }
    });
    li.append(code, btn);
    ul.append(li);
  }
  document.getElementById("subs-empty").hidden = list.length > 0;
}

function setMsg(text, error = false) {
  const el = document.getElementById("subs-msg");
  el.textContent = text;
  el.classList.toggle("error", error);
}

await mountSignIn(document.getElementById("signin"), async (user) => {
  const pay = new URL(stripePaymentLink);
  pay.searchParams.set("client_reference_id", user.uid);
  if (user.email) pay.searchParams.set("prefilled_email", user.email);
  const portal = new URL(stripeCustomerPortal);
  if (user.email) portal.searchParams.set("prefilled_email", user.email);
  document.getElementById("pay").href = pay.toString();
  document.getElementById("portal").href = portal.toString();
  document.getElementById("who").textContent = user.email ?? user.uid;
  document.getElementById("signin").hidden = true;
  document.getElementById("next").hidden = false;

  // The user's own entitlement document (Firestore rules allow reading only your own).
  let ent = {};
  try {
    const snap = await getDoc(doc(getFirestore(app), "entitlements", user.uid));
    ent = snap.exists() ? snap.data() : {};
  } catch {
    ent = {};
  }
  const supporter = ent.plan === "supporter";
  document.getElementById("pay").hidden = supporter;
  document.getElementById("reserve").hidden = !supporter;
  if (supporter) renderSubdomains(ent.subdomains ?? []);

  document.getElementById("reserve-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const input = document.getElementById("sub-name");
    const name = input.value.trim().toLowerCase();
    if (!name) return;
    setMsg("Reserving…");
    try {
      const r = await claim({ name });
      renderSubdomains(r.data.subdomains);
      input.value = "";
      setMsg(`Reserved ${r.data.name}. Pick it in /dev/all when starting a tunnel.`);
    } catch (e) {
      setMsg(e.message, true);
    }
  });
});
