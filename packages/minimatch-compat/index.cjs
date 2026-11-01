// Redocly 1.x expects the pre-v9 callable CommonJS export. Preserve that
// interface while using patched upstream minimatch, including its named API.
const modern = require("minimatch-modern");
module.exports = Object.assign(modern.minimatch, modern);
