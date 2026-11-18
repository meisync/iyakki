// Length, weight and temperature. Factors are to the first unit of each family; temperature has its own rule.
(function () {
  var units = {
    'm': 1, 'km': 1000, 'cm': 0.01, 'mi': 1609.344, 'ft': 0.3048, 'in': 0.0254,
    'kg': 1, 'g': 0.001, 'lb': 0.45359237, 'oz': 0.028349523125,
    'C': 'T', 'F': 'T', 'K': 'T'
  };
  var family = { m: 'L', km: 'L', cm: 'L', mi: 'L', ft: 'L', 'in': 'L', kg: 'W', g: 'W', lb: 'W', oz: 'W', C: 'T', F: 'T', K: 'T' };
  function toC(v, u) { return u === 'C' ? v : u === 'F' ? (v - 32) * 5 / 9 : v - 273.15; }
  function fromC(v, u) { return u === 'C' ? v : u === 'F' ? v * 9 / 5 + 32 : v + 273.15; }
  function convert(v, a, b) {
    if (family[a] !== family[b]) return NaN;
    if (family[a] === 'T') return fromC(toC(v, a), b);
    return v * units[a] / units[b];
  }
  var from = document.getElementById('from'), to = document.getElementById('to');
  Object.keys(units).forEach(function (u) {
    var o = document.createElement('option'); o.value = u; o.textContent = u; from.appendChild(o);
    var p = document.createElement('option'); p.value = u; p.textContent = u; to.appendChild(p);
  });
  to.value = 'km'; from.value = 'm';
  document.getElementById('go').addEventListener('click', function () {
    var v = parseFloat(document.getElementById('value').value);
    var r = convert(v, from.value, to.value);
    document.getElementById('result').textContent = isNaN(r) ? 'Those units do not match' : String(Math.round(r * 1e6) / 1e6) + ' ' + to.value;
  });
  window.__convert = convert;
})();
