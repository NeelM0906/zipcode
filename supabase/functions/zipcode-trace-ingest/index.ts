const FUNCTION_NAME = "zipcode-trace-ingest";
const BUCKET = "zipcode-rollout-traces";
const CAPTURE_POLICY_VERSION = 1;
const MAX_PART_BYTES = 4 * 1024 * 1024;
const MAX_SESSION_JSON_BYTES = 64 * 1024;
const DEFAULT_ZIPCODE_API = "https://olympustest.ngrok.pro/v1";
const UUID_PATTERN =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const SHA256_PATTERN = /^[0-9a-f]{64}$/;
const LOGIN_PATTERN = /^[a-z0-9](?:[a-z0-9-]{0,38})$/;

type JsonRecord = Record<string, unknown>;

class HttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

function json(status: number, body: JsonRecord): Response {
  return Response.json(body, {
    status,
    headers: { "cache-control": "no-store" },
  });
}

function routePath(request: Request): string {
  const pathname = new URL(request.url).pathname;
  const marker = `/${FUNCTION_NAME}`;
  const offset = pathname.indexOf(marker);
  if (offset < 0) return pathname;
  return pathname.slice(offset + marker.length) || "/";
}

function requiredEnvironment(name: string): string {
  const value = Deno.env.get(name);
  if (!value) throw new HttpError(500, `missing ${name}`);
  return value.replace(/\/$/, "");
}

function serviceHeaders(extra: HeadersInit = {}): Headers {
  const serviceKey = requiredEnvironment("SUPABASE_SERVICE_ROLE_KEY");
  const headers = new Headers(extra);
  headers.set("apikey", serviceKey);
  headers.set("authorization", `Bearer ${serviceKey}`);
  return headers;
}

async function serviceFetch(
  path: string,
  init: RequestInit = {},
): Promise<Response> {
  const response = await fetch(
    `${requiredEnvironment("SUPABASE_URL")}${path}`,
    {
      ...init,
      headers: serviceHeaders(init.headers),
    },
  );
  return response;
}

async function responseMessage(response: Response): Promise<string> {
  const text = await response.text();
  if (!text) return `HTTP ${response.status}`;
  try {
    const value = JSON.parse(text) as JsonRecord;
    return String(value.message ?? value.error ?? value.msg ?? text);
  } catch {
    return text.slice(0, 500);
  }
}

async function requireSuccess(
  response: Response,
  operation: string,
): Promise<Response> {
  if (response.ok) return response;
  throw new HttpError(
    response.status >= 500 ? 502 : response.status,
    `${operation}: ${await responseMessage(response)}`,
  );
}

async function authenticate(request: Request): Promise<string> {
  const authorization = request.headers.get("authorization");
  if (!authorization?.toLowerCase().startsWith("bearer ")) {
    throw new HttpError(401, "missing ZIPCODE bearer token");
  }
  const zipcodeApi = (Deno.env.get("ZIPCODE_API_URL") ?? DEFAULT_ZIPCODE_API)
    .replace(/\/$/, "");
  const response = await fetch(`${zipcodeApi}/auth/me`, {
    headers: { authorization },
    signal: AbortSignal.timeout(10_000),
  });
  if (response.status === 401 || response.status === 403) {
    throw new HttpError(401, "invalid or revoked ZIPCODE session");
  }
  await requireSuccess(response, "ZIPCODE identity check failed");
  const body = await response.json() as JsonRecord;
  const login = String(body.github_login ?? "").toLowerCase();
  if (!LOGIN_PATTERN.test(login)) {
    throw new HttpError(401, "ZIPCODE identity was invalid");
  }
  return login;
}

async function ensureBucket(): Promise<void> {
  const current = await serviceFetch(`/storage/v1/bucket/${BUCKET}`);
  if (current.ok) return;
  // Hosted Storage currently returns 400 "Bucket not found" while older
  // releases returned 404 for this lookup.
  if (current.status !== 400 && current.status !== 404) {
    await requireSuccess(current, "read trace bucket");
  }
  const created = await serviceFetch("/storage/v1/bucket", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      id: BUCKET,
      name: BUCKET,
      public: false,
      file_size_limit: MAX_PART_BYTES,
      allowed_mime_types: ["application/octet-stream"],
    }),
  });
  if (!created.ok) {
    // Two first uploads can race. Confirm the bucket exists instead of relying
    // on the Storage API's conflict status, which has varied across releases.
    const raced = await serviceFetch(`/storage/v1/bucket/${BUCKET}`);
    if (!raced.ok) await requireSuccess(created, "create trace bucket");
  }
}

async function restRows(path: string): Promise<JsonRecord[]> {
  const response = await requireSuccess(
    await serviceFetch(`/rest/v1/${path}`, {
      headers: { accept: "application/json" },
    }),
    "read trace metadata",
  );
  return await response.json() as JsonRecord[];
}

async function upsert(
  table: string,
  conflict: string,
  body: JsonRecord,
): Promise<void> {
  await requireSuccess(
    await serviceFetch(
      `/rest/v1/${table}?on_conflict=${encodeURIComponent(conflict)}`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          prefer: "resolution=merge-duplicates,return=minimal",
        },
        body: JSON.stringify(body),
      },
    ),
    `write ${table}`,
  );
}

function textField(body: JsonRecord, name: string, maxLength = 4096): string {
  const value = body[name];
  if (
    typeof value !== "string" || value.length === 0 || value.length > maxLength
  ) {
    throw new HttpError(400, `invalid ${name}`);
  }
  return value;
}

function optionalText(
  body: JsonRecord,
  name: string,
  maxLength = 4096,
): string | null {
  const value = body[name];
  if (value === undefined || value === null || value === "") return null;
  if (typeof value !== "string" || value.length > maxLength) {
    throw new HttpError(400, `invalid ${name}`);
  }
  return value;
}

function integerField(body: JsonRecord, name: string, minimum: number): number {
  const value = body[name];
  if (!Number.isSafeInteger(value) || Number(value) < minimum) {
    throw new HttpError(400, `invalid ${name}`);
  }
  return Number(value);
}

async function createSession(
  request: Request,
  login: string,
): Promise<Response> {
  const rawBody = await request.text();
  if (new TextEncoder().encode(rawBody).byteLength > MAX_SESSION_JSON_BYTES) {
    throw new HttpError(413, "session metadata is too large");
  }
  let body: JsonRecord;
  try {
    body = JSON.parse(rawBody) as JsonRecord;
  } catch {
    throw new HttpError(400, "invalid JSON body");
  }
  if (typeof body !== "object" || body === null || Array.isArray(body)) {
    throw new HttpError(400, "invalid JSON body");
  }
  const traceId = textField(body, "trace_id", 36).toLowerCase();
  const bundleSha256 = textField(body, "bundle_sha256", 64).toLowerCase();
  if (!UUID_PATTERN.test(traceId)) throw new HttpError(400, "invalid trace_id");
  if (!SHA256_PATTERN.test(bundleSha256)) {
    throw new HttpError(400, "invalid bundle_sha256");
  }
  const capturePolicyVersion = integerField(body, "capture_policy_version", 1);
  if (capturePolicyVersion !== CAPTURE_POLICY_VERSION) {
    throw new HttpError(409, "unsupported capture policy version");
  }
  const partCount = integerField(body, "part_count", 1);
  const totalBytes = integerField(body, "total_bytes", 0);
  const acceptedAt = textField(body, "consent_accepted_at", 64);
  const startedAt = textField(body, "started_at", 64);
  const endedAt = optionalText(body, "ended_at", 64);
  const metadata = body.metadata ?? {};
  if (
    typeof metadata !== "object" || metadata === null || Array.isArray(metadata)
  ) {
    throw new HttpError(400, "invalid metadata");
  }

  const existing = await restRows(
    `zipcode_trace_sessions?trace_id=eq.${traceId}` +
      "&select=github_login,bundle_sha256,total_bytes,part_count,status",
  );
  if (existing.length > 0) {
    const current = existing[0];
    if (current.github_login !== login) {
      throw new HttpError(409, "trace_id is already owned");
    }
    if (
      current.bundle_sha256 !== bundleSha256 ||
      Number(current.total_bytes) !== totalBytes ||
      Number(current.part_count) !== partCount
    ) {
      throw new HttpError(
        409,
        "trace_id metadata does not match the existing upload",
      );
    }
    return json(200, { trace_id: traceId, status: String(current.status) });
  }

  await upsert("zipcode_trace_consents", "github_login", {
    github_login: login,
    policy_version: capturePolicyVersion,
    accepted_at: acceptedAt,
    last_seen_at: new Date().toISOString(),
    revoked_at: null,
    metadata: { client: "zipcode" },
  });
  const storagePrefix = `${login}/${traceId}`;
  await upsert("zipcode_trace_sessions", "trace_id", {
    trace_id: traceId,
    rollout_id: textField(body, "rollout_id", 128),
    root_thread_id: textField(body, "root_thread_id", 128),
    github_login: login,
    schema_version: integerField(body, "schema_version", 1),
    capture_policy_version: capturePolicyVersion,
    client_version: textField(body, "client_version", 128),
    status: "uploading",
    started_at: startedAt,
    ended_at: endedAt,
    bundle_sha256: bundleSha256,
    total_bytes: totalBytes,
    part_count: partCount,
    storage_prefix: storagePrefix,
    model: optionalText(body, "model", 256),
    repository_path: optionalText(body, "repository_path"),
    repository_remote: optionalText(body, "repository_remote"),
    repository_commit: optionalText(body, "repository_commit", 128),
    metadata,
  });
  return json(201, { trace_id: traceId, status: "uploading" });
}

async function ownedSession(
  traceId: string,
  login: string,
): Promise<JsonRecord> {
  const rows = await restRows(
    `zipcode_trace_sessions?trace_id=eq.${traceId}` +
      "&select=github_login,status,total_bytes,part_count,storage_prefix,bundle_sha256",
  );
  if (rows.length === 0) throw new HttpError(404, "trace session not found");
  if (rows[0].github_login !== login) {
    throw new HttpError(404, "trace session not found");
  }
  return rows[0];
}

async function sha256Hex(bytes: ArrayBuffer): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Array.from(new Uint8Array(digest))
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

function encodedObjectPath(path: string): string {
  return path.split("/").map(encodeURIComponent).join("/");
}

async function uploadPart(
  request: Request,
  login: string,
  traceId: string,
  partNumber: number,
): Promise<Response> {
  const session = await ownedSession(traceId, login);
  if (session.status === "complete") {
    return json(200, { trace_id: traceId, status: "complete" });
  }
  const partCount = Number(session.part_count);
  if (
    !Number.isSafeInteger(partNumber) || partNumber < 0 ||
    partNumber >= partCount
  ) {
    throw new HttpError(400, "invalid part number");
  }
  const expectedSha256 =
    request.headers.get("x-zipcode-part-sha256")?.toLowerCase() ?? "";
  if (!SHA256_PATTERN.test(expectedSha256)) {
    throw new HttpError(400, "invalid part sha256");
  }
  const buffer = await request.arrayBuffer();
  const bytes = new Uint8Array(buffer);
  if (bytes.byteLength === 0 || bytes.byteLength > MAX_PART_BYTES) {
    throw new HttpError(
      413,
      `part must be between 1 and ${MAX_PART_BYTES} bytes`,
    );
  }
  if (await sha256Hex(buffer) !== expectedSha256) {
    throw new HttpError(400, "part sha256 mismatch");
  }

  await ensureBucket();
  const objectPath = `${session.storage_prefix}/parts/${
    String(partNumber).padStart(6, "0")
  }.bin`;
  await requireSuccess(
    await serviceFetch(
      `/storage/v1/object/${BUCKET}/${encodedObjectPath(objectPath)}`,
      {
        method: "POST",
        headers: {
          "content-type": "application/octet-stream",
          "x-upsert": "true",
        },
        body: bytes,
      },
    ),
    "upload trace part",
  );
  await upsert("zipcode_trace_parts", "trace_id,part_number", {
    trace_id: traceId,
    part_number: partNumber,
    object_path: objectPath,
    size_bytes: bytes.byteLength,
    sha256: expectedSha256,
  });
  return json(200, {
    trace_id: traceId,
    part_number: partNumber,
    size_bytes: bytes.byteLength,
  });
}

async function completeSession(
  login: string,
  traceId: string,
): Promise<Response> {
  const session = await ownedSession(traceId, login);
  const parts = await restRows(
    `zipcode_trace_parts?trace_id=eq.${traceId}` +
      "&select=part_number,size_bytes,sha256&order=part_number.asc",
  );
  const expectedCount = Number(session.part_count);
  if (parts.length !== expectedCount) {
    throw new HttpError(
      409,
      `expected ${expectedCount} parts, received ${parts.length}`,
    );
  }
  let totalBytes = 0;
  for (let index = 0; index < parts.length; index += 1) {
    if (Number(parts[index].part_number) !== index) {
      throw new HttpError(409, `missing trace part ${index}`);
    }
    totalBytes += Number(parts[index].size_bytes);
  }
  if (totalBytes !== Number(session.total_bytes)) {
    throw new HttpError(
      409,
      "uploaded byte count does not match the bundle manifest",
    );
  }
  await requireSuccess(
    await serviceFetch(
      `/rest/v1/zipcode_trace_sessions?trace_id=eq.${traceId}`,
      {
        method: "PATCH",
        headers: {
          "content-type": "application/json",
          prefer: "return=minimal",
        },
        body: JSON.stringify({
          status: "complete",
          completed_at: new Date().toISOString(),
        }),
      },
    ),
    "complete trace upload",
  );
  return json(200, {
    trace_id: traceId,
    status: "complete",
    part_count: expectedCount,
    total_bytes: totalBytes,
    bundle_sha256: String(session.bundle_sha256),
  });
}

async function deleteSession(
  login: string,
  traceId: string,
): Promise<Response> {
  await ownedSession(traceId, login);
  const parts = await restRows(
    `zipcode_trace_parts?trace_id=eq.${traceId}&select=object_path&order=part_number.asc`,
  );
  const paths = parts.map((part) => String(part.object_path));
  for (let offset = 0; offset < paths.length; offset += 1000) {
    await requireSuccess(
      await serviceFetch(`/storage/v1/object/${BUCKET}`, {
        method: "DELETE",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ prefixes: paths.slice(offset, offset + 1000) }),
      }),
      "delete trace objects",
    );
  }
  await requireSuccess(
    await serviceFetch(
      `/rest/v1/zipcode_trace_sessions?trace_id=eq.${traceId}`,
      {
        method: "DELETE",
        headers: { prefer: "return=minimal" },
      },
    ),
    "delete trace metadata",
  );
  return json(200, { trace_id: traceId, status: "deleted" });
}

Deno.serve(async (request: Request): Promise<Response> => {
  try {
    const path = routePath(request);
    if (request.method === "GET" && path === "/health") {
      return json(200, { status: "ok", service: FUNCTION_NAME });
    }
    const login = await authenticate(request);
    if (request.method === "POST" && path === "/sessions") {
      return await createSession(request, login);
    }
    const partMatch = path.match(/^\/sessions\/([0-9a-f-]+)\/parts\/(\d+)$/i);
    if (request.method === "PUT" && partMatch) {
      const traceId = partMatch[1].toLowerCase();
      if (!UUID_PATTERN.test(traceId)) {
        throw new HttpError(400, "invalid trace_id");
      }
      return await uploadPart(request, login, traceId, Number(partMatch[2]));
    }
    const completeMatch = path.match(/^\/sessions\/([0-9a-f-]+)\/complete$/i);
    if (request.method === "POST" && completeMatch) {
      const traceId = completeMatch[1].toLowerCase();
      if (!UUID_PATTERN.test(traceId)) {
        throw new HttpError(400, "invalid trace_id");
      }
      return await completeSession(login, traceId);
    }
    const sessionMatch = path.match(/^\/sessions\/([0-9a-f-]+)$/i);
    if (request.method === "DELETE" && sessionMatch) {
      const traceId = sessionMatch[1].toLowerCase();
      if (!UUID_PATTERN.test(traceId)) {
        throw new HttpError(400, "invalid trace_id");
      }
      return await deleteSession(login, traceId);
    }
    return json(404, { error: "not found" });
  } catch (error) {
    if (error instanceof HttpError) {
      return json(error.status, { error: error.message });
    }
    return json(500, { error: "internal trace ingestion error" });
  }
});
