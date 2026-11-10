import { randomUUID } from "node:crypto";
import { createEnvironmentKeyProvider, decryptCopilotCredential, encryptCopilotCredential } from "./copilot-crypto.mjs";
import { checkCopilotSubscription } from "./copilot-sdk.mjs";

const flows = new Map();
const apiVersion = "2022-11-28";

function accountError(code) {
  const error = new Error("Copilot account is not ready");
  error.code = code;
  return error;
}

export function createCopilotAccounts({ env = process.env, fetchImpl = fetch } = {}) {
  const supabaseUrl = (env.SUPABASE_URL || "").replace(/\/$/, "");
  const serviceKey = env.SUPABASE_SERVICE_ROLE_KEY || "";
  const clientId = env.GITHUB_OAUTH_CLIENT_ID || "";
  const clientSecret = env.GITHUB_OAUTH_CLIENT_SECRET || "";
  const keyProvider = createEnvironmentKeyProvider(env);
  const requireConfig = () => {
    if (!clientId || !clientSecret || !supabaseUrl || !serviceKey) throw new Error("Copilot connection is not configured");
    keyProvider.keyFor(keyProvider.currentKeyId());
  };
  const dbHeaders = () => ({ apikey: serviceKey, authorization: `Bearer ${serviceKey}`, "content-type": "application/json" });

  async function rowsFor(user) {
    const query = new URLSearchParams({
      select: "account_id,github_user_id,github_login,status,subscription_status,encryption_key_id,encrypted_token,wrapped_dek,token_expires_at",
      supabase_user_id: `eq.${user.supabaseUserId}`,
      order: "created_at.asc",
    });
    const response = await fetchImpl(`${supabaseUrl}/rest/v1/copilot_accounts?${query}`, { headers: dbHeaders(), signal: AbortSignal.timeout(10000) });
    if (!response.ok) throw new Error("Copilot accounts unavailable");
    return response.json();
  }

  async function rowsForGithubAccount(githubUserId) {
    const query = new URLSearchParams({
      select: "account_id,supabase_user_id",
      github_user_id: `eq.${githubUserId}`,
      limit: "1",
    });
    const response = await fetchImpl(`${supabaseUrl}/rest/v1/copilot_accounts?${query}`, {
      headers: dbHeaders(), signal: AbortSignal.timeout(10000),
    });
    if (!response.ok) throw new Error("Copilot account ownership could not be checked");
    return response.json();
  }

  async function updateRecord(accountId, values) {
    const response = await fetchImpl(`${supabaseUrl}/rest/v1/copilot_accounts?account_id=eq.${encodeURIComponent(accountId)}`, {
      method: "PATCH", headers: { ...dbHeaders(), prefer: "return=minimal" }, body: JSON.stringify(values), signal: AbortSignal.timeout(10000),
    });
    if (!response.ok) throw new Error("Copilot account update failed");
  }

  async function setStatus(record, status, subscriptionStatus = record.subscription_status) {
    await updateRecord(record.account_id, { status, subscription_status: subscriptionStatus, updated_at: new Date().toISOString() });
    record.status = status;
    record.subscription_status = subscriptionStatus;
  }

  async function getValidCredential(record) {
    let credential = decryptCopilotCredential({
      account_id: record.account_id,
      encryption_key_id: record.encryption_key_id,
      encrypted_token: record.encrypted_token,
      wrapped_dek: record.wrapped_dek,
    }, keyProvider);
    if (credential.expiresAt && Date.parse(credential.expiresAt) < Date.now() + 60000) {
      if (!credential.refreshToken) return null;
      const refreshRequest = {
        client_id: clientId,
        client_secret: clientSecret,
        grant_type: "refresh_token",
        refresh_token: credential.refreshToken,
      };
      const response = await fetchImpl("https://github.com/login/oauth/access_token", {
        method: "POST",
        headers: { accept: "application/json", "content-type": "application/json" },
        body: JSON.stringify(refreshRequest),
        signal: AbortSignal.timeout(10000),
      });
      const refreshed = await response.json().catch(() => ({}));
      if (!response.ok || !refreshed.access_token) return null;
      credential = {
        accessToken: refreshed.access_token,
        refreshToken: refreshed.refresh_token || credential.refreshToken,
        expiresAt: new Date(Date.now() + Number(refreshed.expires_in || 28800) * 1000).toISOString(),
        refreshExpiresAt: new Date(Date.now() + Number(refreshed.refresh_token_expires_in || 15897600) * 1000).toISOString(),
      };
    }
    if (record.encryption_key_id !== keyProvider.currentKeyId() || (credential.expiresAt && credential.expiresAt !== record.token_expires_at)) {
      const encrypted = encryptCopilotCredential(credential, record.account_id, keyProvider);
      await updateRecord(record.account_id, {
        encryption_key_id: encrypted.keyId,
        encrypted_token: { nonce: encrypted.nonce, ciphertext: encrypted.ciphertext, tag: encrypted.tag },
        wrapped_dek: encrypted.wrappedKey,
        token_expires_at: credential.expiresAt,
        updated_at: new Date().toISOString(),
      });
      record.encryption_key_id = encrypted.keyId;
      record.encrypted_token = { nonce: encrypted.nonce, ciphertext: encrypted.ciphertext, tag: encrypted.tag };
      record.wrapped_dek = encrypted.wrappedKey;
      record.token_expires_at = credential.expiresAt;
    }
    return credential;
  }

  async function githubUser(token) {
    const response = await fetchImpl("https://api.github.com/user", {
      headers: { accept: "application/vnd.github+json", authorization: `Bearer ${token}`, "x-github-api-version": apiVersion },
      signal: AbortSignal.timeout(10000),
    });
    if (response.status === 401) return { invalid: true };
    if (response.status === 403) {
      const detail = await response.json().catch(() => ({}));
      const message = typeof detail.message === "string" ? detail.message.toLowerCase() : "";
      if (response.headers.get("x-ratelimit-remaining") === "0" || /rate limit/.test(message)) {
        throw new Error("GitHub account check is rate-limited");
      }
      return { invalid: true };
    }
    if (!response.ok) throw new Error("GitHub account check failed");
    const user = await response.json();
    if (!Number.isSafeInteger(user.id) || typeof user.login !== "string") throw new Error("GitHub account response was invalid");
    return { id: user.id, login: user.login };
  }

  return {
    async start(user) {
      requireConfig();
      for (const [id, flow] of flows) if (flow.expiresAt <= Date.now()) flows.delete(id);
      const params = new URLSearchParams({ client_id: clientId, scope: "offline_access" });
      const response = await fetchImpl("https://github.com/login/device/code", {
        method: "POST", headers: { accept: "application/json", "content-type": "application/x-www-form-urlencoded" }, body: params,
        signal: AbortSignal.timeout(10000),
      });
      const data = await response.json().catch(() => ({}));
      if (!response.ok || !data.device_code || !data.user_code || !data.verification_uri) throw new Error("GitHub device authorization could not start");
      const flowId = randomUUID();
      flows.set(flowId, {
        userId: user.supabaseUserId,
        deviceCode: data.device_code,
        expiresAt: Date.now() + Number(data.expires_in || 900) * 1000,
        interval: Number(data.interval || 5),
        nextPollAt: Date.now(),
      });
      return { flowId, userCode: data.user_code, verificationUri: data.verification_uri, expiresIn: Number(data.expires_in || 900), interval: Number(data.interval || 5) };
    },

    async poll(user, flowId) {
      requireConfig();
      const flow = flows.get(flowId);
      if (!flow || flow.userId !== user.supabaseUserId || flow.expiresAt <= Date.now()) {
        flows.delete(flowId);
        return { status: "expired" };
      }
      if (flow.nextPollAt > Date.now()) return { status: "pending", retryAfter: Math.ceil((flow.nextPollAt - Date.now()) / 1000) };
      const params = new URLSearchParams({ client_id: clientId, device_code: flow.deviceCode, grant_type: "urn:ietf:params:oauth:grant-type:device_code" });
      const response = await fetchImpl("https://github.com/login/oauth/access_token", {
        method: "POST", headers: { accept: "application/json", "content-type": "application/x-www-form-urlencoded" }, body: params,
        signal: AbortSignal.timeout(10000),
      });
      const token = await response.json().catch(() => ({}));
      flow.nextPollAt = Date.now() + flow.interval * 1000;
      if (token.error === "authorization_pending") return { status: "pending", retryAfter: flow.interval };
      if (token.error === "slow_down") { flow.interval += 5; flow.nextPollAt = Date.now() + flow.interval * 1000; return { status: "pending", retryAfter: flow.interval }; }
      if (token.error === "expired_token" || token.error === "access_denied" || !response.ok) {
        flows.delete(flowId);
        return { status: "expired" };
      }
      if (!token.access_token) throw new Error("GitHub authorization did not return an access token");
      const identity = await githubUser(token.access_token);
      if (identity.invalid) { flows.delete(flowId); return { status: "needs_reconnect" }; }
      const linkedRows = await rowsForGithubAccount(identity.id);
      if (linkedRows.some((row) => row.supabase_user_id !== user.supabaseUserId)) {
        flows.delete(flowId);
        throw accountError("copilot_account_already_linked");
      }
      const subscriptionStatus = await checkCopilotSubscription(token.access_token);
      const existing = linkedRows.find((row) => row.supabase_user_id === user.supabaseUserId);
      const accountId = existing?.account_id || randomUUID();
      const credential = {
        accessToken: token.access_token,
        refreshToken: token.refresh_token || null,
        expiresAt: token.expires_in ? new Date(Date.now() + Number(token.expires_in) * 1000).toISOString() : null,
        refreshExpiresAt: token.refresh_token_expires_in ? new Date(Date.now() + Number(token.refresh_token_expires_in) * 1000).toISOString() : null,
      };
      const encrypted = encryptCopilotCredential(credential, accountId, keyProvider);
      const row = {
        account_id: accountId,
        supabase_user_id: user.supabaseUserId,
        github_user_id: identity.id,
        github_login: identity.login,
        status: "connected",
        subscription_status: subscriptionStatus,
        encryption_key_id: encrypted.keyId,
        encrypted_token: { nonce: encrypted.nonce, ciphertext: encrypted.ciphertext, tag: encrypted.tag },
        wrapped_dek: encrypted.wrappedKey,
        token_expires_at: credential.expiresAt,
        updated_at: new Date().toISOString(),
      };
      const saved = await fetchImpl(`${supabaseUrl}/rest/v1/copilot_accounts?on_conflict=supabase_user_id,github_user_id`, {
        method: "POST", headers: { ...dbHeaders(), prefer: "resolution=merge-duplicates,return=minimal" }, body: JSON.stringify(row), signal: AbortSignal.timeout(10000),
      });
      if (!saved.ok) {
        const failure = await saved.json().catch(() => ({}));
        if (failure.code === "23505") throw accountError("copilot_account_already_linked");
        throw new Error("Copilot account could not be saved");
      }
      flows.delete(flowId);
      return { status: "connected", account: { id: accountId, login: identity.login, status: "connected", subscriptionStatus } };
    },

    async list(user) {
      requireConfig();
      const records = await rowsFor(user);
      const accounts = [];
      for (const record of records) {
        if (record.status === "needs_reconnect") {
          accounts.push({ id: record.account_id, login: record.github_login, status: "needs_reconnect", subscriptionStatus: record.subscription_status });
          continue;
        }
        try {
          const credential = await getValidCredential(record);
          if (!credential) {
            await setStatus(record, "needs_reconnect");
          } else {
            const identity = await githubUser(credential.accessToken);
            if (identity.invalid) await setStatus(record, "needs_reconnect");
            else if (record.status !== "connected") await setStatus(record, "connected");
          }
        } catch {
          // Network or storage failures are not evidence of revocation.
        }
        accounts.push({ id: record.account_id, login: record.github_login, status: record.status, subscriptionStatus: record.subscription_status });
      }
      return accounts;
    },

    async providerAccounts(user) {
      requireConfig();
      const query = new URLSearchParams({
        select: "account_id,github_login,status,subscription_status",
        supabase_user_id: `eq.${user.supabaseUserId}`,
        order: "created_at.asc",
      });
      const response = await fetchImpl(`${supabaseUrl}/rest/v1/copilot_accounts?${query}`, {
        headers: dbHeaders(), signal: AbortSignal.timeout(5000),
      });
      if (!response.ok) throw new Error("Copilot accounts unavailable");
      const records = await response.json();
      return records.map((record) => ({
        id: record.account_id,
        login: record.github_login,
        status: record.status,
        subscriptionStatus: record.subscription_status,
      }));
    },

    async disconnect(user, accountId) {
      requireConfig();
      const response = await fetchImpl(`${supabaseUrl}/rest/v1/copilot_accounts?account_id=eq.${encodeURIComponent(accountId)}&supabase_user_id=eq.${encodeURIComponent(user.supabaseUserId)}`, {
        method: "DELETE", headers: dbHeaders(), signal: AbortSignal.timeout(10000),
      });
      if (!response.ok) throw new Error("Copilot account could not be disconnected");
    },

    async providerCredential(user, accountId) {
      requireConfig();
      const records = await rowsFor(user);
      const record = records.find((candidate) => candidate.account_id === accountId);
      if (!record) throw accountError("copilot_account_not_found");
      if (record.status === "needs_reconnect") throw accountError("copilot_needs_reconnect");
      if (record.subscription_status === "inactive") throw accountError("copilot_subscription_inactive");
      if (record.subscription_status !== "active") throw accountError("copilot_subscription_unverified");
      try {
        const credential = await getValidCredential(record);
        if (!credential) {
          await setStatus(record, "needs_reconnect");
          throw accountError("copilot_needs_reconnect");
        }
        const identity = await githubUser(credential.accessToken);
        if (identity.invalid) {
          await setStatus(record, "needs_reconnect");
          throw accountError("copilot_needs_reconnect");
        }
        return { accessToken: credential.accessToken, login: record.github_login };
      } catch (error) {
        if (error?.code) throw error;
        throw accountError("copilot_service_unavailable");
      }
    },

    async markProviderStatus(user, accountId, status) {
      if (status !== "needs_reconnect" && status !== "inactive") return;
      const records = await rowsFor(user);
      const record = records.find((candidate) => candidate.account_id === accountId);
      if (!record) return;
      if (status === "needs_reconnect") await setStatus(record, status);
      else await updateRecord(accountId, { subscription_status: "inactive", updated_at: new Date().toISOString() });
    },
  };
}
