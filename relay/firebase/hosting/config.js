// TODO(owner): fill in after creating the Firebase project and the Stripe product (relay/README.md).
// None of these are secrets: the Web API key only identifies the Firebase project.
export const firebaseConfig = {
  apiKey: "TODO-firebase-web-api-key",
  authDomain: "TODO-firebase-project-id.firebaseapp.com",
  projectId: "TODO-firebase-project-id",
  appId: "TODO-firebase-app-id",
};

// Stripe Payment Link for the Supporter subscription (~$4/month). The upgrade page appends
// ?client_reference_id=<uid> so the stripeWebhook function knows which account paid.
export const stripePaymentLink = "https://buy.stripe.com/TODO";

// Stripe customer portal login link (Stripe Dashboard → Settings → Billing → Customer portal).
export const stripeCustomerPortal = "https://billing.stripe.com/p/login/TODO";
