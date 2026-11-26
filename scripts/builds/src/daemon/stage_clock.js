// A staged page's clock, which the stage puts before the page's own scripts:
// the time stands at the fixture's `now` (@@NOW@@, seconds since 1970) and
// reads in UTC, so every shot of a fixture words each age and time of day the
// same on every day and host.
(() => {
  const Real = Date;
  const now = @@NOW@@ * 1000;
  class Staged extends Real {
    constructor(...args) { super(...(args.length ? args : [now])); }
    static now() { return now; }
    // A date and time with no zone, as a claim's date is, reads as UTC too.
    static parse(text) {
      return Real.parse(/^\d{4}-\d\d-\d\dT\d\d:\d\d$/.test(text) ? text + "Z" : text);
    }
    toLocaleString(locales, options) { return super.toLocaleString(locales, {...options, timeZone: "UTC"}); }
    toLocaleTimeString(locales, options) { return super.toLocaleTimeString(locales, {...options, timeZone: "UTC"}); }
  }
  for (const part of ["FullYear", "Month", "Date", "Hours", "Minutes"]) {
    Staged.prototype["get" + part] = Real.prototype["getUTC" + part];
  }
  Staged.prototype.setHours = Real.prototype.setUTCHours;
  globalThis.Date = Staged;
})();
