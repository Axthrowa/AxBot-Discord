// Panel teması ve vurgu rengi: seçim tarayıcıda saklanır, sayfa çizilmeden önce <html>'e uygulanır
// (yanıp sönme olmasın diye <head>'de, eşzamanlı yüklenir). Seçici düğmeleri: [data-pick] içindeki [data-value].
(function () {
  var root = document.documentElement;
  var keys = { theme: "axbot-tema", accent: "axbot-renk" };
  function apply(kind, value) {
    var attr = kind === "theme" ? "data-theme" : "data-accent";
    if (value) root.setAttribute(attr, value); else root.removeAttribute(attr);
  }
  function load(kind) {
    try { return localStorage.getItem(keys[kind]) || ""; } catch (e) { return ""; }
  }
  apply("theme", load("theme"));
  apply("accent", load("accent"));

  document.addEventListener("DOMContentLoaded", function () {
    document.querySelectorAll("[data-pick]").forEach(function (group) {
      var kind = group.getAttribute("data-pick");
      var mark = function (value) {
        group.querySelectorAll("[data-value]").forEach(function (b) {
          var on = b.getAttribute("data-value") === value;
          b.classList.toggle("on", on);
          b.setAttribute("aria-pressed", on ? "true" : "false");
        });
      };
      mark(load(kind));
      group.addEventListener("click", function (e) {
        var button = e.target.closest("[data-value]");
        if (!button) return;
        var value = button.getAttribute("data-value");
        apply(kind, value);
        try { if (value) localStorage.setItem(keys[kind], value); else localStorage.removeItem(keys[kind]); } catch (err) {}
        mark(value);
      });
    });
  });
})();
