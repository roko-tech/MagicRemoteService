// Polyfill for util.isDate removed in newer Node.js versions.
// Loaded via node --require before WebOS CLI commands (ssh2 SFTP needs it).
var util = require('util');
if (typeof util.isDate !== 'function') {
	util.isDate = function (obj) {
		return Object.prototype.toString.call(obj) === '[object Date]';
	};
}
