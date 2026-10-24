// Shared sign-in widget: Google, GitHub and passwordless email link (Firebase JS SDK from gstatic).
import { firebaseConfig } from "./config.js";
import { initializeApp } from "https://www.gstatic.com/firebasejs/13.1.0/firebase-app.js";
import {
  getAuth,
  setPersistence,
  browserSessionPersistence,
  GoogleAuthProvider,
  GithubAuthProvider,
  signInWithPopup,
  signInWithRedirect,
  getRedirectResult,
  sendSignInLinkToEmail,
  isSignInWithEmailLink,
  signInWithEmailLink,
  signOut,
} from "https://www.gstatic.com/firebasejs/13.1.0/firebase-auth.js";

export const app = initializeApp(firebaseConfig);
export const auth = getAuth(app);
export { signOut };

const EMAIL_KEY = "devall.emailForSignIn";

function message(el, text, error = false) {
  el.textContent = text;
  el.classList.toggle("error", error);
}

/**
 * Wires the provider buttons inside `root` and calls `onUser(user)` once signed in.
 * The web session only lives for this tab (session persistence).
 */
export async function mountSignIn(root, onUser) {
  await setPersistence(auth, browserSessionPersistence);
  const msg = root.querySelector(".msg");
  const done = async (user) => {
    try {
      await onUser(user);
    } catch (e) {
      message(msg, String(e?.message ?? e), true);
      root.hidden = false;
    }
  };

  // Coming back from a redirect sign-in or an email link.
  try {
    const r = await getRedirectResult(auth);
    if (r?.user) return done(r.user);
  } catch (e) {
    message(msg, e.message, true);
  }
  if (isSignInWithEmailLink(auth, location.href)) {
    let email = localStorage.getItem(EMAIL_KEY);
    if (!email) email = window.prompt("Confirm your email address to finish signing in");
    if (email) {
      try {
        const r = await signInWithEmailLink(auth, email, location.href);
        localStorage.removeItem(EMAIL_KEY);
        return done(r.user);
      } catch (e) {
        message(msg, e.message, true);
      }
    }
  }

  const popup = async (provider) => {
    message(msg, "Opening the sign-in window…");
    try {
      const r = await signInWithPopup(auth, provider);
      await done(r.user);
    } catch (e) {
      if (e?.code === "auth/popup-blocked" || e?.code === "auth/operation-not-supported-in-this-environment") {
        await signInWithRedirect(auth, provider);
      } else {
        message(msg, e.message, true);
      }
    }
  };
  root.querySelector("[data-google]").addEventListener("click", () => popup(new GoogleAuthProvider()));
  root.querySelector("[data-github]").addEventListener("click", () => popup(new GithubAuthProvider()));
  root.querySelector("[data-email-form]").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const email = root.querySelector("[data-email]").value.trim();
    if (!email) return;
    try {
      // The link returns to this exact URL (with the app's port and state).
      await sendSignInLinkToEmail(auth, email, { url: location.href, handleCodeInApp: true });
      localStorage.setItem(EMAIL_KEY, email);
      message(msg, `Check ${email} for a sign-in link and open it in this browser.`);
    } catch (e) {
      message(msg, e.message, true);
    }
  });
  root.hidden = false;
}
