import "./signin.css";
import { Bridge } from "./core/bridge";

const root = document.getElementById("root");
if (root) {
  root.innerHTML = `
    <section class="signin-card" aria-labelledby="title">
      <div class="brand-mark" aria-hidden="true">m</div>
      <p class="eyebrow">MACHA</p>
      <h1 id="title">Sign in to sync your devices from anywhere</h1>
      <button id="continue" class="primary" type="button">Continue with Google</button>
      <button id="cancel" class="cancel" type="button" hidden>Cancel sign-in</button>
      <button id="skip" class="skip" type="button">Skip for now</button>
      <p class="later">You can sign in later from Settings.</p>
      <p id="message" role="status" aria-live="polite"></p>
    </section>`;

  const continueButton = document.querySelector<HTMLButtonElement>("#continue")!;
  const cancelButton = document.querySelector<HTMLButtonElement>("#cancel")!;
  const skipButton = document.querySelector<HTMLButtonElement>("#skip")!;
  const message = document.querySelector<HTMLElement>("#message")!;
  let busy = false;
  let configured = false;

  const updateButtons = () => {
    continueButton.disabled = busy || !configured;
    skipButton.disabled = false;
    cancelButton.hidden = !busy;
    continueButton.textContent = busy ? "Waiting for Google sign-in…" : "Continue with Google";
  };

  const signIn = async () => {
    if (busy) return;
    if (!configured) {
      message.textContent = "Remote sync is not available in this build. Choose Skip for now to use nearby pairing.";
      return;
    }
    busy = true;
    message.textContent = "Opening your system browser…";
    updateButtons();
    try {
      const status = await Bridge.remoteSignIn();
      if (!status.signedIn) throw new Error("Google sign-in did not complete. Please try again.");
      message.textContent = "Signed in. Finishing device setup…";
      await Bridge.remoteFinishSignInGate();
    } catch (error) {
      const details = String(error);
      const status = await Bridge.remoteStatus().catch(() => null);
      if (status?.signedIn) {
        message.textContent = `Google sign-in succeeded, but device setup needs attention: ${details}`;
        await Bridge.remoteFinishSignInGate().catch(() => undefined);
        return;
      }
      message.textContent = details.includes("cancelled") || details.includes("timed out")
        ? "Sign-in was cancelled. You can try again or skip for now."
        : `Sign-in could not finish: ${details}`;
    } finally {
      busy = false;
      updateButtons();
    }
  };

  continueButton.addEventListener("click", () => void signIn());
  cancelButton.addEventListener("click", () => {
    void Bridge.remoteCancelSignIn().catch(() => undefined);
    message.textContent = "Cancelling sign-in…";
  });
  skipButton.addEventListener("click", () => {
    skipButton.disabled = true;
    void Bridge.remoteSkipSignIn().catch((error: unknown) => {
      message.textContent = `Could not continue: ${String(error)}`;
      skipButton.disabled = false;
    });
  });

  void Bridge.remoteStatus().then((status) => {
    configured = status.configured;
    if (status.signedIn) {
      void Bridge.remoteFinishSignInGate();
      return;
    }
    if (!configured) {
      message.textContent = "Remote sync is not available in this build. You can still skip and use nearby pairing.";
    }
    updateButtons();
  }).catch((error: unknown) => {
    message.textContent = `Could not load sign-in status: ${String(error)}`;
  });
}
