<script>
  import { onMount } from "svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { api } from "./api.js";

  let { onConnected } = $props();

  let mode = $state("usb"); // usb | wifi
  let step = $state(0); // 0 wake · 1 connect · 2 done
  let pinned = $state(false); // a step was clicked, or the reader told us where it is: stop the loop
  let ports = $state([]);
  let found = $state([]);
  let message = $state(""); // a hint or an error, replaces the default status
  let connecting = $state(""); // shown in the status line
  let busy = false; // an attempt is in flight (quiet or not)
  let manualHost = $state("crosspoint.local");
  let showOtherPorts = $state(false);

  const STEPS = {
    usb: [
      { title: "Wake your reader", text: "Press the power button and stay on the home screen." },
      { title: "Plug it into your Mac", text: "Use the USB‑C or POGO cable." },
      { title: "Done", text: "X Studio connects on its own." },
    ],
    wifi: [
      { title: "Wake your reader", text: "Press the power button and open the main menu." },
      { title: "Start File Transfer", text: "Keep the reader on the same Wi‑Fi as your Mac." },
      { title: "Done", text: "Pick it below or type its address." },
    ],
  };
  const steps = $derived(STEPS[mode]);
  const likelyPorts = $derived(ports.filter((p) => p.likelyReader));
  const otherPorts = $derived(ports.filter((p) => !p.likelyReader));

  const status = $derived.by(() => {
    if (connecting) return `Connecting to ${connecting.replace("/dev/", "")}…`;
    if (message) return message;
    if (mode === "wifi") return found.length ? `Found ${found.length === 1 ? "a reader" : `${found.length} readers`} on your network.` : "Looking for readers on your network…";
    return "Waiting for your reader…";
  });

  function setMode(m) {
    mode = m;
    step = 0;
    pinned = false;
    message = "";
    if (m === "wifi") pollWifi();
  }

  // Ports we tried and should not retry on our own until they are unplugged.
  const gaveUp = new Set();

  async function tryUsb(port, manual = false) {
    if (connecting || busy) return;
    busy = true;
    // Background retries while we wait for the home screen stay quiet: no status flicker.
    const quiet = !manual && !!message;
    if (!quiet) {
      connecting = port;
      message = "";
    }
    try {
      onConnected(await api.connectUsb(port));
    } catch (e) {
      const text = String(e);
      if (text.includes("home screen")) {
        message = "Your reader is plugged in but not on the home screen. Open it and X Studio will connect.";
        step = 0;
        pinned = true;
      } else {
        message = text;
        if (!manual) gaveUp.add(port);
      }
    } finally {
      connecting = "";
      busy = false;
    }
  }

  async function tryWifi(host, wsPort = null) {
    if (connecting || !host) return;
    connecting = host;
    message = "";
    try {
      onConnected(await api.connectWifi(host, wsPort));
    } catch (e) {
      message = `Couldn’t connect to ${host}: ${e}`;
    } finally {
      connecting = "";
    }
  }

  async function pollUsb() {
    try {
      ports = (await api.listSerialPorts()) ?? [];
    } catch {
      return;
    }
    for (const name of [...gaveUp]) if (!ports.some((p) => p.name === name)) gaveUp.delete(name);
    const candidate = likelyPorts.find((p) => !gaveUp.has(p.name));
    if (candidate) tryUsb(candidate.name);
  }

  let discovering = false;
  async function pollWifi() {
    if (discovering) return;
    discovering = true;
    try {
      found = (await api.discoverWifi()) ?? [];
    } catch {
    } finally {
      discovering = false;
    }
  }

  onMount(() => {
    const anim = setInterval(() => {
      if (!pinned) step = (step + 1) % 3;
    }, 2800);
    pollUsb();
    const poll = setInterval(() => (mode === "usb" ? pollUsb() : pollWifi()), 2500);
    return () => {
      clearInterval(anim);
      clearInterval(poll);
    };
  });
</script>

<div class="connect">
  <!-- Illustration: a reader that wakes, connects, and confirms -->
  <div class="scene {mode} s{step}" aria-hidden="true">
    <svg class="cable" viewBox="0 0 380 300" fill="none">
      <path d="M120 262 C120 296 300 296 300 228" pathLength="1" />
    </svg>

    <div class="waves"><span></span><span></span><span></span></div>

    <div class="reader">
      <span class="btn left a"></span>
      <span class="btn left b"></span>
      <span class="btn power"></span>
      <span class="press"></span>
      <div class="screen">
        <div class="layer sleep">
          <span class="z">z</span><span class="z">z</span>
        </div>
        <div class="layer home">
          <div class="home-cover"></div>
          <div class="lines"><i></i><i></i><i style="width: 60%"></i></div>
          <div class="tiles"><b></b><b></b><b></b><b></b></div>
        </div>
        <div class="layer menu">
          <p>Library</p><p>Settings</p><p class="hit">File Transfer</p><p>OPDS</p>
        </div>
        <div class="layer connected">
          <span class="tick">
            <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"><path d="M5 12.5l4.5 4.5L19 7.5"></path></svg>
          </span>
          <span class="done-text">Connected</span>
        </div>
      </div>
      <span class="plug"></span>
    </div>

    <div class="mac">
      <div class="mac-screen"><span class="mac-tick">✓</span></div>
      <div class="mac-base"></div>
    </div>
  </div>

  <!-- Timeline -->
  <ol class="timeline">
    {#each steps as s, i (s.title)}
      <li class:current={i === step} class:done={i < step}>
        <div class="step">
          <span class="node">
            {#if i < step}
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3.5" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5"></path></svg>
            {:else}{i + 1}{/if}
          </span>
          <strong>{s.title}</strong>
          <span class="text">{s.text}</span>
        </div>
      </li>
    {/each}
  </ol>

  <!-- Status -->
  <div class="status" aria-live="polite">
    <p class="status-line" class:warn={!!message && !connecting}>
      {#if !message || connecting}<span class="spinner"></span>{/if}
      {status}
    </p>

    {#if mode === "usb"}
      {#if showOtherPorts}
        <div class="choices">
          {#each otherPorts as p (p.name)}
            <button class="choice dim" onclick={() => tryUsb(p.name, true)} disabled={!!connecting}>
              <span class="mono">{p.name.replace("/dev/", "")}</span> <small>{p.product || p.manufacturer || "USB"}</small>
            </button>
          {/each}
        </div>
      {/if}
      <p class="alt">
        {#if otherPorts.length}
          <button class="link" onclick={() => (showOtherPorts = !showOtherPorts)}>{showOtherPorts ? "Hide" : "Show"} other ports ({otherPorts.length})</button>
          ·
        {/if}
        <button class="link" onclick={() => setMode("wifi")}>Connect over Wi‑Fi instead</button>
      </p>
    {:else}
      {#if found.length}
        <div class="choices">
          {#each found as d (d.ip)}
            <button class="choice" onclick={() => tryWifi(d.ip, d.wsPort)} disabled={!!connecting}>
              <span class="dot"></span>{d.hostname} <small class="mono">{d.ip}</small>
            </button>
          {/each}
        </div>
      {/if}
      <form class="manual" onsubmit={(e) => (e.preventDefault(), tryWifi(manualHost.trim()))}>
        <label class="sr" for="host">Reader address</label>
        <input id="host" bind:value={manualHost} placeholder="192.168.1.48" spellcheck="false" autocomplete="off" />
        <button type="submit" disabled={!!connecting || !manualHost.trim()}>Connect</button>
      </form>
      <p class="alt"><button class="link" onclick={() => setMode("usb")}>Use a cable instead</button></p>
    {/if}
  </div>

  <!-- CrossInk -->
  <p class="crossink">
    <svg width="14" height="14" viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" d="M12 21s-7.5-4.6-9.6-9.2C.9 8.4 3 4.5 6.7 4.5c2.2 0 3.6 1.2 4.3 2.4h2c.7-1.2 2.1-2.4 4.3-2.4 3.7 0 5.8 3.9 4.3 7.3C19.5 16.4 12 21 12 21z"></path></svg>
    Works best with <button class="link" onclick={() => openUrl("https://inky.crossink.dev/#flash-tools")}>CrossInk</button>
  </p>
</div>

<style>
  .connect {
    min-height: 100%; box-sizing: border-box; padding: 32px 24px;
    display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 30px;
    overflow-y: auto;
  }

  /* ---------- illustration ---------- */
  .scene { position: relative; width: 380px; height: 300px; flex: none; }

  .reader {
    position: absolute; left: 50px; top: 14px; width: 140px; height: 236px; box-sizing: border-box;
    padding: 12px 10px 22px; background: #2b2a27; border-radius: 16px;
    box-shadow: 0 24px 44px -26px rgb(0 0 0 / 0.55);
    transition: transform 0.7s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .screen { position: relative; width: 100%; height: 100%; background: #f2f0ea; border-radius: 3px; overflow: hidden; }

  /* side buttons (X4 Classic has them on both sides; power on the right) */
  .btn { position: absolute; width: 4px; background: #57544e; border-radius: 2px; }
  .btn.left { left: -3px; height: 24px; }
  .btn.left.a { top: 70px; }
  .btn.left.b { top: 100px; }
  .btn.power { right: -3px; top: 56px; height: 30px; transition: transform 0.15s; }
  .press { position: absolute; right: -14px; top: 61px; width: 20px; height: 20px; border-radius: 50%; border: 2px solid var(--ink); opacity: 0; }

  .layer { position: absolute; inset: 0; box-sizing: border-box; padding: 12px 10px; opacity: 0; transition: opacity 0.5s; }
  .sleep { background: #e4e1d8; display: flex; align-items: flex-start; justify-content: flex-end; gap: 2px; padding: 16px; color: #8e8a80; font-family: var(--serif); font-style: italic; }
  .sleep .z:first-child { font-size: 13px; margin-top: 8px; }
  .sleep .z:last-child { font-size: 18px; }
  .home { display: flex; flex-direction: column; gap: 9px; }
  .home-cover { height: 82px; border-radius: 3px; background: #6a6660; }
  .lines { display: flex; flex-direction: column; gap: 5px; }
  .lines i { display: block; height: 3px; border-radius: 2px; background: #b9b4a8; }
  .tiles { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; margin-top: 2px; }
  .tiles b { height: 26px; border: 1.5px solid #8e8a80; border-radius: 4px; }
  .menu p { margin: 0 0 7px; padding: 5px 7px; border-radius: 4px; font-size: 10px; color: #3d3a35; }
  .menu .hit { background: #1c1b19; color: #f2f0ea; }
  .connected { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 8px; background: rgb(242 240 234 / 0.94); }
  .tick { width: 40px; height: 40px; border-radius: 50%; background: #1c1b19; color: #f2f0ea; display: grid; place-items: center; transform: scale(0.4); transition: transform 0.45s cubic-bezier(0.3, 1.6, 0.5, 1); }
  .done-text { font-size: 11px; font-weight: 600; color: #1c1b19; letter-spacing: 0.02em; }

  /* step 0: sleeping screen, the power button is pressed, the home screen fades in */
  .s0 .sleep { opacity: 1; }
  .s0 .home, .s0 .menu { animation: wake 2.8s ease forwards; }
  .s0 .btn.power { animation: push 2.8s ease; }
  .s0 .press { animation: ring 2.8s ease-out; }
  @keyframes wake { 0%, 38% { opacity: 0; } 55%, 100% { opacity: 1; } }
  @keyframes push { 0%, 12% { transform: none; } 18%, 30% { transform: translateX(-3px); } 36%, 100% { transform: none; } }
  @keyframes ring { 0%, 14% { opacity: 0; transform: scale(0.5); } 22% { opacity: 1; } 42%, 100% { opacity: 0; transform: scale(1.6); } }
  .usb .menu { display: none; }
  .wifi .home { display: none; }

  /* step 1 and 2: awake */
  .s1 .home, .s2 .home, .s1 .menu, .s2 .menu { opacity: 1; }
  .s2 .connected { opacity: 1; }
  .s2 .tick { transform: scale(1); }

  /* cable: drawn from the reader to the Mac */
  .cable { position: absolute; inset: 0; width: 100%; height: 100%; overflow: visible; }
  .cable path { stroke: #5c5952; stroke-width: 5; stroke-linecap: round; stroke-dasharray: 1; stroke-dashoffset: 1; transition: stroke-dashoffset 0.2s; }
  .usb.s1 .cable path { animation: draw 1.3s cubic-bezier(0.4, 0, 0.2, 1) 0.3s forwards; }
  .usb.s2 .cable path { stroke-dashoffset: 0; }
  @keyframes draw { to { stroke-dashoffset: 0; } }
  .wifi .cable { display: none; }
  .plug { position: absolute; left: 50%; bottom: -12px; width: 22px; height: 14px; margin-left: -11px; border-radius: 3px 3px 2px 2px; background: #5c5952; opacity: 0; transform: translateY(8px); transition: opacity 0.3s, transform 0.4s; }
  .usb.s1 .plug, .usb.s2 .plug { opacity: 1; transform: none; }

  /* Wi‑Fi: arcs between reader and Mac */
  .waves { position: absolute; left: 212px; top: 112px; width: 30px; height: 60px; display: none; }
  .wifi .waves { display: block; }
  .waves span { position: absolute; top: 50%; left: 0; border: 3px solid transparent; border-right-color: var(--ink); border-radius: 50%; transform: translateY(-50%); opacity: 0.12; }
  .waves span:nth-child(1) { width: 14px; height: 14px; left: -4px; }
  .waves span:nth-child(2) { width: 32px; height: 32px; left: -14px; }
  .waves span:nth-child(3) { width: 50px; height: 50px; left: -24px; }
  .wifi.s1 .waves span, .wifi.s2 .waves span { animation: wave 1.4s ease-in-out infinite; }
  .wifi.s1 .waves span:nth-child(2), .wifi.s2 .waves span:nth-child(2) { animation-delay: 0.18s; }
  .wifi.s1 .waves span:nth-child(3), .wifi.s2 .waves span:nth-child(3) { animation-delay: 0.36s; }
  @keyframes wave { 0%, 100% { opacity: 0.12; } 50% { opacity: 1; } }

  /* the Mac */
  .mac { position: absolute; left: 248px; top: 152px; width: 104px; opacity: 0.3; transition: opacity 0.5s; }
  .mac-screen { height: 66px; border: 3px solid #2b2a27; border-radius: 6px 6px 2px 2px; background: #fbfaf6; display: grid; place-items: center; }
  .mac-base { height: 7px; margin: 0 -10px; border-radius: 0 0 6px 6px; background: #2b2a27; }
  .mac-tick { width: 26px; height: 26px; border-radius: 50%; background: var(--ok); color: #fff; display: grid; place-items: center; font-size: 15px; font-weight: 700; transform: scale(0); transition: transform 0.45s cubic-bezier(0.3, 1.6, 0.5, 1); }
  .s1 .mac, .s2 .mac { opacity: 1; }
  .s2 .mac-tick { transform: scale(1); }

  @media (prefers-reduced-motion: reduce) {
    .scene *, .scene *::before { animation: none !important; transition: none !important; }
    .s0 .home, .s0 .menu { opacity: 1; }
    .usb.s1 .cable path { stroke-dashoffset: 0; }
  }

  /* ---------- timeline ---------- */
  .timeline { list-style: none; margin: 0; padding: 0; width: 100%; max-width: 640px; display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); }
  .timeline li { position: relative; display: flex; }
  /* connector to the next node */
  .timeline li:not(:last-child)::after {
    content: ""; position: absolute; top: 15px; left: calc(50% + 20px); right: calc(-50% + 20px); height: 2px;
    background: var(--rule-strong); border-radius: 1px;
  }
  .timeline li.done::after { background: var(--ink); }
  /* Steps are informational only: not clickable, no hover or focus states. */
  .step { flex: 1; display: flex; flex-direction: column; align-items: center; gap: 4px; padding: 0 10px 4px; text-align: center; cursor: default; }
  .node {
    width: 30px; height: 30px; border-radius: 50%; display: grid; place-items: center; margin-bottom: 6px;
    border: 2px solid var(--rule-strong); background: var(--paper); color: var(--ink-3); font-size: 12px; font-weight: 600;
    transition: background 0.3s, border-color 0.3s, color 0.3s, transform 0.3s;
  }
  .timeline strong { font-size: 13px; color: var(--ink-2); transition: color 0.3s; }
  .text { font-size: 12px; color: var(--ink-3); line-height: 1.4; max-width: 190px; }
  li.done .node { border-color: var(--ink); color: var(--ink); }
  li.current .node { background: var(--ink); border-color: var(--ink); color: var(--on-accent); transform: scale(1.08); }
  li.current strong { color: var(--ink); }
  li.current .text { color: var(--ink-2); }

  /* ---------- status ---------- */
  /* fixed height so the centered stack doesn't jump when the status changes */
  .status { width: 100%; max-width: 520px; min-height: 132px; display: flex; flex-direction: column; align-items: center; gap: 10px; text-align: center; }
  .status-line { margin: 0; display: flex; align-items: center; justify-content: center; gap: 9px; font-size: 14px; color: var(--ink); min-height: 22px; }
  .status-line.warn { padding: 8px 14px; border-radius: 10px; background: color-mix(in srgb, #e3b23c 18%, transparent); }
  .choices { display: flex; flex-wrap: wrap; justify-content: center; gap: 8px; }
  .choice { display: flex; align-items: center; gap: 8px; min-height: 36px; padding: 0 14px; border-radius: 999px; border-color: var(--rule-strong); }
  .choice small { color: var(--ink-3); }
  .choice.dim { border-style: dashed; }
  .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--ok); flex: none; }
  .manual { display: flex; gap: 8px; width: 100%; max-width: 360px; }
  .manual input { flex: 1; min-height: 36px; }
  .manual button { min-height: 36px; padding: 0 16px; }
  .alt { margin: 0; color: var(--ink-3); font-size: 12px; }
  .alt .link { font-size: 12px; }

  /* ---------- CrossInk ---------- */
  .crossink { margin: 6px 0 0; display: flex; align-items: center; gap: 6px; color: var(--ink-3); font-size: 12px; }
  .crossink svg { color: #c0503a; }
  .crossink .link { padding: 0; font-size: 12px; color: var(--ink-2); }

  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); }

  @media (max-width: 560px) {
    .scene { transform: scale(0.8); margin: -30px 0; }
    .timeline { grid-template-columns: 1fr; gap: 14px; }
    .timeline li::after { display: none; }
  }
</style>
