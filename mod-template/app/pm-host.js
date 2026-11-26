// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later
var PmHost = (function(exports) {
	Object.defineProperty(exports, Symbol.toStringTag, { value: "Module" });
	var MAX_FRAME_HEIGHT = 4096;
	/** The codes the page itself answers with or reads; the backend answers with the rest. A test in
	*  error.rs pins each to the backend's code of the same name. `needsPrompt` is the one the page
	*  turns into PM's first-use question; `notGranted` is what a mod is told after the person says no
	*  to it. The lines from `aiCapReached` on are the experimental calls' (PR B), listed so a page-side
	*  answer for `ai.complete`, `net.fetch`, `secrets.set` or `notify` names a code by word and the
	*  same test holds it to the backend's; their numbers freeze at the first release after PR C. The
	*  last three are PR C's: `needsConfirmation` is the one PM's page turns into its own yes/no question
	*  (answered with `mod_confirm`), and `declined` is what the mod is told after a no. */
	var MOD_ERRORS = {
		unknownMethod: "MOD-1001",
		badArgs: "MOD-1003",
		notGranted: "MOD-2001",
		needsPrompt: "MOD-2002",
		rateLimited: "MOD-3001",
		tooLarge: "MOD-3002",
		internal: "MOD-9001",
		aiCapReached: "MOD-3004",
		callTimedOut: "MOD-3005",
		noModel: "MOD-5004",
		priceUnknown: "MOD-5005",
		modelCallFailed: "MOD-5006",
		hostNotAllowed: "MOD-6001",
		hostNotPublic: "MOD-6002",
		hostUnresolved: "MOD-6003",
		networkUnavailable: "MOD-6004",
		tooManyRedirects: "MOD-6005",
		responseTooLarge: "MOD-6006",
		secretNotSet: "MOD-6007",
		engineNotReady: "MOD-5007",
		needsConfirmation: "MOD-2003",
		declined: "MOD-2004"
	};
	/** The calls PM's page performs once the backend has authorised them (`api::PageAction`), because
	*  only the page holds what they need. A test in handlers.rs pins this list to the backend's. */
	var PAGE_ACTIONS = [
		"theme.get",
		"ui.openDocument",
		"secrets.set",
		"notify",
		"ui.openTaskEditor"
	];
	/** The events PM tells a frame (`api::events::EVENTS`; a test in events.rs pins this list to the
	*  backend's). The backend sends only these, and only to a mod that may hear them. */
	var MOD_EVENTS = [
		"documents.added",
		"documents.deleted",
		"theme.changed",
		"grants.changed",
		"mod.paused",
		"session.closed",
		"tasks.changed",
		"projects.renamed",
		"projects.merged",
		"projects.deleted",
		"calendar.synced",
		"lifecycle.suspend"
	];
	/** Method names: what the host API's dotted names look like, bounded. Rust checks each against its
	*  dispatch table; this only keeps junk off the IPC channel. */
	var METHOD = /^[A-Za-z][A-Za-z0-9._-]{0,127}$/;
	/** Whether `name` is shaped like a method name. One test for both ends of the port: the page refuses
	*  what fails it, and the frame SDK (src/kit/host.ts) refuses it first, so a typo costs no token. */
	function isMethodName(name) {
		return typeof name === "string" && METHOD.test(name);
	}
	/** What `internal` always says: what went wrong inside PM stays in PM. */
	var INTERNAL_MESSAGE = "PM couldn't complete that call.";
	Object.freeze({
		pm: "port",
		version: 1
	});
	/** The binary layout's version, and its fixed header (four little-endian u32s: version, count,
	*  dimension, reserved); `src-tauri/src/mods/api/embeddings.rs` holds the layout. */
	var VECTORS_VERSION = 1;
	var VECTORS_HEADER_BYTES = 16;
	/** The layout is little-endian and the views read the machine's byte order, so a big-endian machine
	*  (none PM runs on) would read garbage: it refuses instead. */
	var LITTLE_ENDIAN = new Uint8Array(new Uint16Array([1]).buffer)[0] === 1;
	/** Whether `data` is an ArrayBuffer, by its tag, so one from another realm (the webview's IPC layer, a
	*  test's) counts. */
	function isArrayBuffer(data) {
		return Object.prototype.toString.call(data) === "[object ArrayBuffer]";
	}
	/** The vectors answer, checked against its own header: the right version, the length exactly the
	*  header's arithmetic, inside the cap. `null` for anything else, which is PM's failure, never passed
	*  on. */
	function readVectors(buffer) {
		if (!LITTLE_ENDIAN) return null;
		const size = buffer.byteLength;
		if (size < VECTORS_HEADER_BYTES || size > 20971520) return null;
		const header = new DataView(buffer);
		const [version, count, dimension, reserved] = [
			0,
			4,
			8,
			12
		].map((at) => header.getUint32(at, true));
		if (version !== VECTORS_VERSION || reserved !== 0) return null;
		const floats = count * dimension;
		if (floats > size) return null;
		if (size !== VECTORS_HEADER_BYTES + 8 * count + 4 * floats) return null;
		return {
			count,
			dimension,
			ids: new Float64Array(buffer, VECTORS_HEADER_BYTES, count),
			vectors: new Float32Array(buffer, VECTORS_HEADER_BYTES + 8 * count, floats)
		};
	}
	/** Messages arrive structured-cloned: plain data, with no getters or prototypes of the sender's. */
	function isRecord$1(value) {
		return typeof value === "object" && value !== null && !Array.isArray(value);
	}
	/** A requested height held to PM's bounds; `null` for anything that isn't a finite number. */
	function clampFrameHeight(requested) {
		if (typeof requested !== "number" || !Number.isFinite(requested)) return null;
		return Math.min(MAX_FRAME_HEIGHT, Math.max(64, Math.round(requested)));
	}
	/** Whether `text` is at most `cap` UTF-8 bytes. Its byte length is never less than its UTF-16 length
	*  and at most three times it, so the encoder runs only when the answer could fall either side. */
	function fitsBytes(text, cap) {
		if (text.length > cap) return false;
		if (text.length * 3 <= cap) return true;
		return new TextEncoder().encode(text).length <= cap;
	}
	/** A refusal the page makes itself. */
	function refusal(code, message) {
		return {
			code: MOD_ERRORS[code],
			message
		};
	}
	/** A call's arguments as exactly the JSON text PM will send, measured in bytes. Structured clone
	*  carries more than JSON (a BigInt, a cycle), and none of it is an argument PM's API takes. */
	function encodeArgs(args) {
		let json;
		try {
			json = JSON.stringify(args === void 0 ? null : args);
		} catch {
			return {
				ok: false,
				error: refusal("badArgs", "A call's arguments must be plain JSON.")
			};
		}
		if (!fitsBytes(json, 1048576)) return {
			ok: false,
			error: refusal("tooLarge", "A call's arguments are too large.")
		};
		return {
			ok: true,
			json
		};
	}
	var CODE = /^MOD-\d{4}$/;
	/** The backend's answer, checked: anything not in its shape is PM's own failure, never passed on. Raw
	*  bytes are the vectors; everything else is the JSON reply. */
	function readReply(data) {
		const broken = {
			kind: "error",
			error: refusal("internal", INTERNAL_MESSAGE)
		};
		if (isArrayBuffer(data)) {
			const vectors = readVectors(data);
			return vectors ? {
				kind: "binary",
				vectors,
				buffer: data
			} : broken;
		}
		if (!isRecord$1(data)) return broken;
		switch (data.kind) {
			case "value": return "value" in data ? {
				kind: "value",
				value: data.value
			} : broken;
			case "page": {
				const action = data.action;
				if (typeof action !== "string") return broken;
				if (!PAGE_ACTIONS.includes(action)) return {
					kind: "error",
					error: refusal("unknownMethod", "PM has no call by that name.")
				};
				return {
					kind: "page",
					action,
					args: data.args
				};
			}
			case "error": {
				const e = data.error;
				if (!isRecord$1(e) || typeof e.code !== "string" || !CODE.test(e.code)) return broken;
				if (typeof e.message !== "string") return broken;
				const error = {
					code: e.code,
					message: e.message
				};
				if (typeof e.permission === "string") error.permission = e.permission;
				if (typeof e.revision === "number" && Number.isSafeInteger(e.revision)) error.revision = e.revision;
				return {
					kind: "error",
					error
				};
			}
			default: return broken;
		}
	}
	/** An event from the backend's channel, checked: a name PM knows and a sequence number. Anything
	*  else is PM's own failure and is dropped (it can't be acknowledged, so it can't be told of). */
	function readEvent(raw) {
		if (!isRecord$1(raw)) return null;
		const { name, seq, payload, missed } = raw;
		if (typeof name !== "string" || !MOD_EVENTS.includes(name)) return null;
		if (typeof seq !== "number" || !Number.isSafeInteger(seq) || seq < 1) return null;
		const event = {
			event: name,
			seq,
			payload: payload ?? null
		};
		if (typeof missed === "number" && Number.isSafeInteger(missed) && missed > 0) event.missed = missed;
		return event;
	}
	/** Where a frame's events wait until its port exists, and how they go to it after: in order, each
	*  checked. The backend never has more than `EVENT_BUFFER` outstanding, so the hold can't pass that
	*  unless the backend is wrong; if it does, the excess is dropped and the next event says so. */
	var EventPump = class {
		held = [];
		connected = false;
		lost = 0;
		post;
		constructor(post) {
			this.post = post;
		}
		/** An event from the backend. */
		push(raw) {
			const event = readEvent(raw);
			if (!event) return;
			if (this.lost > 0) {
				event.missed = (event.missed ?? 0) + this.lost;
				this.lost = 0;
			}
			if (this.connected) this.post(event);
			else if (this.held.length >= 256) this.lost += 1 + (event.missed ?? 0);
			else this.held.push(event);
		}
		/** The frame's port exists: send what was held, in order, then everything from here on. */
		connect() {
			if (this.connected) return;
			this.connected = true;
			const held = this.held;
			this.held = [];
			for (const event of held) this.post(event);
		}
	};
	// src/kit/host.ts
	/** Calls the SDK keeps out at once: the backend's own per-mod cap (`MAX_IN_FLIGHT` in limits.rs), and
	*  half the page's flood line, which counts a call held for a first-use question too. */
	var MAX_CONCURRENT_CALLS = 16;
	/** The pace calls are sent at, per second sustained and in a burst: under the backend's bucket (50 a
	*  second, burst 100) so it never refuses one, and under the page's flood line (the same 50) with
	*  room for the acknowledgements that draw on it. */
	var PACE_PER_SECOND = 40;
	var PACE_BURST = 80;
	/** Calls the SDK will hold waiting. A mod that queues more has a bug, and finds out. */
	var MAX_QUEUED_CALLS = 1e3;
	/** How long events may sit unacknowledged, and how many: the batch an acknowledgement covers. Well
	*  inside the backend's window of 256, so a frame that keeps up never has an event dropped. */
	var ACK_DELAY_MS = 50;
	var ACK_AFTER = 64;
	/** How long `connectHost` waits for PM's port. PM answers at once; a page that is not PM's frame, or
	*  whose session ended before it started, hears nothing. */
	var CONNECT_TIMEOUT_MS = 1e4;
	/** PM refused a call (or failed it), with its stable code (ERROR_CODES.md). `permission` is the one a
	*  refusal is about and `revision` the current one after a stale write, when PM said so. */
	var ModCallError = class extends Error {
		code;
		permission;
		revision;
		constructor(error) {
			super(error.message);
			this.name = "ModCallError";
			this.code = error.code;
			if (error.permission !== void 0) this.permission = error.permission;
			if (error.revision !== void 0) this.revision = error.revision;
		}
	};
	var HostError = class extends Error {
		kind;
		constructor(kind, message) {
			super(message);
			this.name = "HostError";
			this.kind = kind;
		}
	};
	/** The environment of a real page: its parent window, its message events, the platform's clock. An
	*  error from a handler is rethrown from a task of its own, so it reaches the page's error reporting
	*  without unwinding the SDK. */
	function windowEnv(win) {
		return {
			parent: win.parent === win ? null : win.parent,
			listen(handler) {
				const listener = (event) => handler(event);
				win.addEventListener("message", listener);
				return () => win.removeEventListener("message", listener);
			},
			now: () => performance.now(),
			setTimer: (run, ms) => setTimeout(run, ms),
			clearTimer: (handle) => clearTimeout(handle),
			report: (error) => {
				queueMicrotask(() => {
					throw error;
				});
			}
		};
	}
	function isRecord(value) {
		return typeof value === "object" && value !== null && !Array.isArray(value);
	}
	/** The pace calls are sent at, in thousandths of a call so refilling is exact. The caller says what
	*  time it is. An acknowledgement spends a token unconditionally (it must not wait, or a slow frame
	*  would have events dropped), so the bucket can go into debt, bounded, and calls wait it out. */
	var Pacer = class {
		milli = 8e4;
		last;
		constructor(now) {
			this.last = now;
		}
		refill(now) {
			const elapsed = Math.max(0, now - this.last);
			this.last = Math.max(this.last, now);
			this.milli = Math.min(8e4, this.milli + elapsed * 40);
		}
		take(now) {
			this.refill(now);
			if (this.milli < 1e3) return false;
			this.milli -= 1e3;
			return true;
		}
		spend(now) {
			this.refill(now);
			this.milli = Math.max(-8e4, this.milli - 1e3);
		}
		/** Milliseconds until a call may go, at least 1. */
		waitMs(now) {
			this.refill(now);
			return Math.max(1, Math.ceil((1e3 - this.milli) / 40));
		}
	};
	/** The reason `session.closed` carries, when it is a short string. */
	function closeReason(payload) {
		const reason = isRecord(payload) ? payload.reason : void 0;
		return typeof reason === "string" && reason.length <= 32 ? reason : null;
	}
	/** The vectors answer as the frame receives it, checked. PM transfers views over one buffer, so
	*  `ids` and `vectors` are typed arrays whose lengths must agree with `count` and `dimension`; a raw
	*  buffer in the bridge's layout is decoded the same way the page does. Anything else is PM's own
	*  failure, and says so as MOD-9001 rather than handing the mod a half-read answer. */
	function readDocumentVectors(value) {
		const broken = () => new ModCallError(refusal("internal", INTERNAL_MESSAGE));
		if (isArrayBuffer(value)) {
			const vectors = readVectors(value);
			if (!vectors) throw broken();
			return vectors;
		}
		if (!isRecord(value)) throw broken();
		const { count, dimension, ids, vectors } = value;
		const tag = (v) => Object.prototype.toString.call(v);
		if (typeof count !== "number" || typeof dimension !== "number" || !Number.isSafeInteger(count) || !Number.isSafeInteger(dimension) || count < 0 || dimension < 0 || tag(ids) !== "[object Float64Array]" || tag(vectors) !== "[object Float32Array]") throw broken();
		const idView = ids;
		const vectorView = vectors;
		if (idView.length !== count || vectorView.length !== count * dimension) throw broken();
		return {
			count,
			dimension,
			ids: idView,
			vectors: vectorView
		};
	}
	/** One connected frame. Built only by `connectWith`, once PM's port exists. */
	var Connection = class {
		env;
		port;
		pacer;
		queue = [];
		pending = /* @__PURE__ */ new Map();
		handlers = /* @__PURE__ */ new Set();
		/** Events wait here until a handler exists, and are told to handlers in order after. */
		events = new EventPump((event) => this.deliver(event));
		nextId = 1;
		pumpTimer = null;
		ackTimer = null;
		lastSeq = 0;
		ackedSeq = 0;
		lastHeight = null;
		end = null;
		constructor(env, port) {
			this.env = env;
			this.port = port;
			this.pacer = new Pacer(env.now());
			port.onmessage = (event) => this.onMessage(event.data);
		}
		get closed() {
			return this.end;
		}
		call(method, args) {
			if (this.end) return Promise.reject(this.end);
			if (!isMethodName(method)) return Promise.reject(new ModCallError(refusal("badArgs", "That is not a method name PM could read.")));
			const encoded = encodeArgs(args);
			if (!encoded.ok) return Promise.reject(new ModCallError(encoded.error));
			if (this.queue.length >= 1e3) return Promise.reject(new HostError("queue-full", "Too many calls are waiting to be sent to PM."));
			return new Promise((resolve, reject) => {
				this.queue.push({
					method,
					json: encoded.json,
					resolve,
					reject
				});
				this.pump();
			});
		}
		async documentVectors(options = {}) {
			return readDocumentVectors(await this.call("embeddings.documentVectors", options));
		}
		onEvent(handler) {
			this.handlers.add(handler);
			this.events.connect();
			return () => {
				this.handlers.delete(handler);
			};
		}
		resize(height) {
			if (typeof height !== "number") throw new TypeError("resize takes a number of pixels");
			const clamped = clampFrameHeight(height);
			if (clamped === null) throw new RangeError("resize takes a finite number of pixels");
			if (this.end || clamped === this.lastHeight) return;
			this.lastHeight = clamped;
			this.post({ resize: clamped });
		}
		flushed() {
			if (this.end) return;
			this.pacer.spend(this.env.now());
			this.post({ flushed: true });
		}
		/** Send what the limits allow, and when the pace is what stops it, come back when a token exists. */
		pump() {
			if (this.end) return;
			while (this.queue.length > 0 && this.pending.size < 16) {
				const now = this.env.now();
				if (!this.pacer.take(now)) {
					this.pumpLater(this.pacer.waitMs(now));
					return;
				}
				const next = this.queue.shift();
				if (!next) return;
				const id = this.nextId++;
				this.pending.set(id, next);
				if (!this.post({
					id,
					method: next.method,
					args: JSON.parse(next.json)
				})) return;
			}
		}
		pumpLater(ms) {
			if (this.pumpTimer !== null) return;
			this.pumpTimer = this.env.setTimer(() => {
				this.pumpTimer = null;
				this.pump();
			}, ms);
		}
		/** A message to PM. A port that throws is a port that is gone. */
		post(message) {
			try {
				this.port.postMessage(message);
				return true;
			} catch {
				this.close(new HostError("closed", "PM's connection to this mod ended."));
				return false;
			}
		}
		onMessage(data) {
			if (this.end || !isRecord(data)) return;
			if (typeof data.id === "number" && "ok" in data) this.onReply(data.id, data);
			else if (typeof data.event === "string") this.takeEvent(data);
		}
		onReply(id, data) {
			const call = this.pending.get(id);
			if (!call) return;
			this.pending.delete(id);
			if (data.ok === true) call.resolve(data.result);
			else {
				const reply = readReply({
					kind: "error",
					error: data.error
				});
				call.reject(new ModCallError(reply.kind === "error" ? reply.error : refusal("internal", INTERNAL_MESSAGE)));
			}
			this.pump();
		}
		takeEvent(data) {
			const { event: name, seq, payload, missed } = data;
			if (typeof seq !== "number" || !Number.isSafeInteger(seq) || seq < 1) return;
			if (seq <= this.lastSeq) return;
			this.lastSeq = seq;
			this.events.push({
				name,
				seq,
				payload,
				missed
			});
			if (name === "session.closed") {
				const reason = closeReason(payload);
				this.close(new HostError("closed", `PM ended this mod's session${reason ? ` (${reason})` : ""}.`));
				return;
			}
			if (name === "mod.paused") {
				this.close(new HostError("paused", "PM paused this mod."));
				return;
			}
			this.noteEvent();
		}
		/** One event, in order, to every handler. */
		deliver(raw) {
			const event = {
				name: raw.event,
				seq: raw.seq,
				payload: raw.payload
			};
			if (raw.missed !== void 0) event.missed = raw.missed;
			for (const handler of [...this.handlers]) try {
				handler(event);
			} catch (error) {
				this.env.report(error);
			}
		}
		/** One more event taken: acknowledge when enough have piled up, else after a short quiet. */
		noteEvent() {
			if (this.lastSeq - this.ackedSeq >= 64) this.ack();
			else if (this.ackTimer === null) this.ackTimer = this.env.setTimer(() => {
				this.ackTimer = null;
				this.ack();
			}, 50);
		}
		/** Acknowledge everything taken so far. Reached only with an event not yet acknowledged and a host
		*  that is there: the timer starts on such an event, and `close` clears it. */
		ack() {
			if (this.ackTimer !== null) {
				this.env.clearTimer(this.ackTimer);
				this.ackTimer = null;
			}
			this.pacer.spend(this.env.now());
			this.ackedSeq = this.lastSeq;
			this.post({ ack: this.lastSeq });
		}
		/** The host is gone: nothing more will be answered, so everything waiting is rejected now. */
		close(error) {
			if (this.end) return;
			this.end = error;
			for (const timer of [this.pumpTimer, this.ackTimer]) if (timer !== null) this.env.clearTimer(timer);
			this.pumpTimer = null;
			this.ackTimer = null;
			this.port.onmessage = null;
			this.port.close();
			const waiting = [...this.pending.values(), ...this.queue];
			this.pending.clear();
			this.queue.length = 0;
			for (const call of waiting) call.reject(error);
		}
	};
	/** Ask PM for this page's port, in `env`. Rejects with a `HostError` when this page is not in a frame
	*  or PM does not answer in time. Every call asks, so a caller that may run twice uses
	*  `connectHost()`, which asks once. */
	function connectWith(env) {
		return new Promise((resolve, reject) => {
			const parent = env.parent;
			if (!parent) {
				reject(new HostError("not-framed", "This page is not running inside PM."));
				return;
			}
			let timer = null;
			const stop = env.listen((event) => {
				const data = event.data;
				if (event.source !== parent) return;
				if (!isRecord(data) || data.pm !== "port" || data.version !== 1) return;
				if (event.ports.length !== 1) return;
				stop();
				env.clearTimer(timer);
				resolve(new Connection(env, event.ports[0]));
			});
			timer = env.setTimer(() => {
				stop();
				reject(new HostError("no-port", "PM did not answer this mod's request to connect."));
			}, CONNECT_TIMEOUT_MS);
			try {
				parent.postMessage({
					pm: "connect",
					version: 1
				}, "*");
			} catch {
				stop();
				env.clearTimer(timer);
				reject(new HostError("no-port", "PM did not answer this mod's request to connect."));
			}
		});
	}
	var shared = null;
	/** The host of this page, connected once: a second call returns the same promise (PM answers a
	*  frame's request only once per session, so a second ask would hear nothing). */
	function connectHost() {
		if (shared === null) shared = typeof window === "undefined" ? Promise.reject(new HostError("not-framed", "This page is not running inside PM.")) : connectWith(windowEnv(window));
		return shared;
	}
	exports.ACK_AFTER = ACK_AFTER;
	exports.ACK_DELAY_MS = ACK_DELAY_MS;
	exports.CONNECT_TIMEOUT_MS = CONNECT_TIMEOUT_MS;
	exports.HostError = HostError;
	exports.MAX_CONCURRENT_CALLS = MAX_CONCURRENT_CALLS;
	exports.MAX_QUEUED_CALLS = MAX_QUEUED_CALLS;
	exports.ModCallError = ModCallError;
	exports.PACE_BURST = PACE_BURST;
	exports.PACE_PER_SECOND = PACE_PER_SECOND;
	exports.connectHost = connectHost;
	exports.connectWith = connectWith;
	exports.readDocumentVectors = readDocumentVectors;
	exports.windowEnv = windowEnv;
	return exports;
})({});
