(function () {
  var LINK = /^(https?|tel|mailto|sms|geo):|^www\.\S+$|^[\w-]+(\.[\w-]+)*\.[a-z]{2,}(\/\S*)?$/i;
  var STREET = /\b(st|street|ave|avenue|rd|road|blvd|dr|drive|ln|lane|way|hwy|pkwy|ct|pl)\b\.?/i;
  var ZIP = /\b\d{5}(-\d{4})?\b/;

  function detect(v) {
    v = v.trim();
    if (LINK.test(v)) return "link";
    if (/^\d+\s+\S/.test(v) && (STREET.test(v) || v.indexOf(",") >= 0 || ZIP.test(v))) {
      return "map";
    }
    return "note";
  }

  document.querySelectorAll("select[data-autotag]").forEach(function (select) {
    var info = select.form.querySelector("[name=value]");
    select.addEventListener("change", function () {
      select.dataset.touched = "1";
    });
    info.addEventListener("input", function () {
      if (!select.dataset.touched) select.value = detect(info.value);
    });
  });
})();
