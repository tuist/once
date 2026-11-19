const { greeting } = require('@acme/core');

exports.banner = name => `<h1>${greeting(name)}</h1>`;
