/*
 * The waitlist form.
 *
 * Progressive in the only way that matters here: the form posts JSON and replaces itself with
 * a line of text, and if this file fails to load the form still has a required email field and
 * a submit button that does nothing, which is a broken form rather than a form that looks like
 * it worked. There is no server-rendered fallback because there is no server-rendered page.
 */

(function () {
  const form = document.getElementById("wl");
  if (!form) return;
  const msg = document.getElementById("wl-msg");
  const button = form.querySelector("button[type=submit]");

  function say(text, bad) {
    msg.textContent = text;
    msg.className = "wl-msg" + (bad ? " bad" : " ok");
    form.email.setAttribute("aria-invalid", bad ? "true" : "false");
  }

  // The commonest slips, caught before they become an address nobody can reach.
  const TYPOS = { "gmial.com": "gmail.com", "gmal.com": "gmail.com", "gmai.com": "gmail.com", "gmail.co": "gmail.com",
    "hotmial.com": "hotmail.com", "outlok.com": "outlook.com", "yahooo.com": "yahoo.com", "icloud.co": "icloud.com" };

  // Typing again clears the complaint: an error should not shout at a fix in progress.
  form.email.addEventListener("input", () => {
    if (msg.classList.contains("bad")) { msg.textContent = ""; msg.className = "wl-msg"; form.email.removeAttribute("aria-invalid"); }
  });

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const email = form.email.value.trim();
    // The same loose check the Worker does, so the common typo is caught without a round trip.
    if (!/^[^@\s]+@[^@\s.]+\.[^@\s]{2,}$/.test(email)) {
      say(email ? "That does not look like an email address. Check for a missing @ or dot." : "Enter your email to join.", true);
      form.email.focus();
      return;
    }
    const domain = email.split("@")[1].toLowerCase();
    if (TYPOS[domain] && form.dataset.checked !== email) {
      form.dataset.checked = email;
      form.email.value = email.split("@")[0] + "@" + TYPOS[domain];
      say(`Did you mean @${TYPOS[domain]}? We fixed it. Press join again if that is right.`, true);
      return;
    }

    button.disabled = true;
    const was = button.textContent;
    button.textContent = "Joining...";
    try {
      const r = await fetch("/api/waitlist", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          email,
          website: form.website.value,
          source: "site",
        }),
      });
      const j = await r.json().catch(() => ({}));
      if (!r.ok) {
        say(j.error || "That did not work. Try again, or mail hello@kaviri.dev.", true);
        return;
      }
      if (j.already) {
        say("\u2713 You are already on the list. Nothing more to do.", false);
      } else {
        say(`\u2713 You are on the list. A welcome email is on its way to ${email}.`, false);
      }
      form.classList.add("done");
    } catch {
      say("That did not work. Try again, or mail hello@kaviri.dev.", true);
    } finally {
      button.disabled = false;
      button.textContent = was;
    }
  });
})();
