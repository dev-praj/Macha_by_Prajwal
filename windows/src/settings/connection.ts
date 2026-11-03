import { Bridge, type RemoteDevice, type RemoteStatus } from "../core/bridge";
import { h, clear } from "../views/dom";

async function confirmationCode(a: string, b: string): Promise<string> {
  const material = `macha-remote-device-confirm-v1\n${[a, b].sort().join("\n")}`;
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(material)));
  const value = (digest[0]! * 0x1000000) + (digest[1]! << 16) + (digest[2]! << 8) + digest[3]!;
  const digits = String(value % 100_000_000).padStart(8, "0");
  return `${digits.slice(0, 4)}-${digits.slice(4)}`;
}

export function connectionSection(): HTMLElement {
  const section = h("section", { class: "hub-connection" },
    h("h2", {}, h("span", { text: "Connection" })),
    h("p", { class: "hint", text: "Nearby QR pairing and Remote account linking are separate. QR-paired devices continue using the local connection only." }),
  );
  const statusLine = h("p", { class: "hint", role: "status", "aria-live": "polite" });
  const account = h("div", {});
  section.append(account, statusLine);
  let current: RemoteStatus | null = null;
  let devices: RemoteDevice[] = [];
  let busy = false;

  const setMessage = (message: string) => { statusLine.textContent = message; };
  const paint = () => {
    clear(account);
    account.append(h("p", { class: "hint", text: "Sign in with Google to link your devices through Macha Remote. Nearby QR pairing remains separate." }));
    if (!current?.configured) {
      account.append(
        h("p", { class: "hint", text: "Remote sync is not available in this build. Nearby QR pairing is still available." }),
      );
      return;
    }
    if (!current.signedIn) {
      const signIn = h("button", { class: "primary", type: "button", text: "Sign in with Google" });
      signIn.addEventListener("click", () => void run("Opening Google sign-in in your browser…", async () => {
        try {
          current = await Bridge.remoteSignIn();
        } catch (error) {
          // OAuth can finish and persist the session even if the follow-up device
          // registration RPC fails. Reflect the signed-in state and let refresh retry it.
          current = await Bridge.remoteStatus();
          if (!current.signedIn) throw error;
          setMessage("Google sign-in succeeded. Device setup will retry when you refresh.");
        }
        devices = current.signedIn ? await Bridge.remoteDevices() : [];
      }));
      account.append(h("h3", { text: "Google account" }), h("p", { class: "hint", text: "Not signed in" }), signIn);
      if (busy) {
        const cancel = h("button", { type: "button", text: "Cancel sign-in" });
        cancel.addEventListener("click", () => void Bridge.remoteCancelSignIn().catch(() => undefined));
        account.append(cancel);
      }
      account.append(h("p", { class: "hint", text: "Sign in on this laptop and your phone with the same Google account. No QR code is used for Remote mode." }));
      return;
    }

    account.append(
      h("h3", { text: "Google account" }),
      h("p", { class: "hint", text: `${current.email ?? "Signed in"} · This laptop: ${current.deviceStatus ?? "registered"}` }),
    );
    const refresh = h("button", { type: "button", text: "Refresh devices" });
    refresh.disabled = busy;
    refresh.addEventListener("click", () => void run("Refreshing device list…", async () => { await refreshAll(); }));
    const signOut = h("button", { type: "button", text: "Sign out" });
    signOut.disabled = busy;
    signOut.addEventListener("click", () => {
      if (!window.confirm("Sign out of Google on this laptop? Its device approval will be kept; revoke it separately if you want to remove its access.")) return;
      void run("Signing out…", async () => { await Bridge.remoteSignOut(); devices = []; current = await Bridge.remoteStatus(); });
    });
    const accountActions = h("div", { class: "row" }, refresh, signOut);
    const own = devices.find((device) => device.id === current?.deviceId);
    if (own?.status === "approved") {
      const revokeOwn = h("button", { class: "danger", type: "button", text: "Revoke this laptop" });
      revokeOwn.disabled = busy;
      revokeOwn.addEventListener("click", () => {
        if (!window.confirm("Revoke this laptop’s Macha Remote access? You will need to sign in again and have an approved device approve it.")) return;
        void run("Revoking this laptop…", async () => { await Bridge.remoteRevokeDevice(own.id); await refreshAll(); });
      });
      accountActions.append(revokeOwn);
    }
    if (current.deviceStatus === "revoked") {
      const reauthorize = h("button", { class: "primary", type: "button", text: "Re-enroll this laptop" });
      reauthorize.disabled = busy;
      reauthorize.addEventListener("click", () => void run("Reconnecting this laptop…", async () => {
        devices = await Bridge.remoteDevices();
        current = await Bridge.remoteStatus();
      }));
      accountActions.append(reauthorize);
    }
    account.append(accountActions, h("h3", { text: "My devices" }));
    if (!devices.length) account.append(h("p", { class: "hint", text: "No devices are registered yet." }));
    if (own && own.status !== "approved" && !devices.some((device) => device.id !== own.id && device.status === "approved")) {
      account.append(h("p", { class: "hint", text: "No approved device is available to confirm this laptop. Run the Remote device recovery migration in your Supabase project, then refresh devices; an untrusted device will not be approved automatically while another approved device exists." }));
    }
    for (const device of devices) {
      const row = h("div", { class: "remote-device-row" },
        h("strong", { text: `${device.name} · ${device.type}` }),
        h("span", { class: "hint", text: `${device.status}${device.last_seen ? ` · Last seen ${new Date(device.last_seen).toLocaleString()}` : ""}` }),
      );
      if (device.status === "pending" && own) {
        let confirmationCodeValue: string | null = null;
        const code = h("p", { class: "hint", text: "Checking confirmation code…" });
        void confirmationCode(own.pubkey, device.pubkey).then((value) => {
          confirmationCodeValue = value;
          if (code.isConnected) code.textContent = `Compare this code on both devices: ${value}`;
        });
        row.append(code);
        if (own.status === "approved") {
          const approve = h("button", { class: "primary", type: "button", text: "Confirm code and approve" });
          const deny = h("button", { class: "danger", type: "button", text: "Deny" });
          approve.disabled = deny.disabled = busy;
          approve.addEventListener("click", () => {
            if (!confirmationCodeValue) { setMessage("The confirmation code is still loading. Try again in a moment."); return; }
            if (!window.confirm(`Compare this code with the device you are approving. Approve only if they match.\n\n${confirmationCodeValue}`)) return;
            void run("Approving device…", async () => {
            await Bridge.remoteApproveDevice(device.id);
            await refreshAll();
            });
          });
          deny.addEventListener("click", () => void run("Denying device…", async () => {
            await Bridge.remoteDenyDevice(device.id);
            await refreshAll();
          }));
          row.append(h("div", { class: "row" }, approve, deny));
        } else {
          row.append(h("span", { class: "hint", text: "Waiting for approval from an approved device." }));
        }
      }
      if (device.status === "approved" && device.id !== current.deviceId) {
        const revoke = h("button", { class: "danger", type: "button", text: "Revoke" });
        revoke.disabled = busy;
        revoke.addEventListener("click", () => {
          if (!window.confirm(`Revoke ${device.name} from Remote mode?`)) return;
          void run("Revoking device…", async () => { await Bridge.remoteRevokeDevice(device.id); await refreshAll(); });
        });
        row.append(revoke);
      }
      account.append(row);
    }
    const erase = h("button", { class: "danger", type: "button", text: "Delete my account and data" });
    erase.disabled = busy;
    erase.addEventListener("click", () => {
      if (!window.confirm("Delete the Google-linked Macha account and all Remote device/message data? This cannot be undone.")) return;
      void run("Deleting account data…", async () => { await Bridge.remoteDeleteAccount(); devices = []; current = await Bridge.remoteStatus(); });
    });
    account.append(erase);
  };

  const refreshAll = async () => {
    current = await Bridge.remoteStatus();
    if (!current.signedIn) {
      devices = [];
      paint();
      return;
    }
    try {
      devices = await Bridge.remoteDevices();
    } catch (error) {
      // An invalid refresh token is cleared by the native layer. Re-read status so
      // this panel immediately changes from stale "signed in" to the sign-in button.
      const latest = await Bridge.remoteStatus().catch(() => null);
      if (!latest || latest.signedIn) throw error;
      current = latest;
      devices = [];
      paint();
      throw new Error("Your saved Google sign-in expired. Sign in with Google again.");
    }
    // The timer retries after transient network failures. Clear the prior
    // failure once Auth and the device list have both refreshed successfully.
    setMessage("");
    paint();
  };
  const run = async (pending: string, action: () => Promise<void>) => {
    if (busy) return;
    busy = true;
    setMessage(pending);
    paint();
    try { await action(); setMessage(""); }
    catch (error) { setMessage(`Could not update Remote connection: ${String(error)}`); }
    finally { busy = false; paint(); }
  };

  void refreshAll().catch((error: unknown) => setMessage(`Could not load Remote status: ${String(error)}`));
  const timer = window.setInterval(() => {
    if (!section.isConnected) { window.clearInterval(timer); return; }
    if (current?.signedIn && !busy && document.visibilityState === "visible") {
      void refreshAll().catch((error: unknown) => setMessage(`Could not refresh Remote devices: ${String(error)}`));
    }
  }, 12_000);
  return section;
}
