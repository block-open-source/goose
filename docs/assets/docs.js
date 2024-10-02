const backgrounds = [
  "/assets/bg.png",
  "/assets/bg2.png",
  "/assets/bg3.png",
  "/assets/bg4.png",
];

// this is to preload the images so the transition is smooth.
// otherwise, on transition image will flicker without smooth transition.

var hiddenContainer           = document.createElement('div');
hiddenContainer.style.display = 'none';
document.body.appendChild(hiddenContainer);

let index = 1;
for (let bg of backgrounds) {
  let img        = [];
  img[index]     = new Image();
  img[index].src = bg;

  hiddenContainer.appendChild(img[index]);
  index++;
}

function shuffleBackgrounds() {
  let images = []; // preload
  let index  = 1;
  for (let bg of shuffle(backgrounds)) {
    document.body.style.setProperty("--bg" + index, "url(" + bg + ")");
    index++;
  }
}

function shuffle(array) {
  let currentIndex = array.length, randomIndex;

  // While there remain elements to shuffle.
  while (currentIndex !== 0) {

    // Pick a remaining element.
    randomIndex = Math.floor(Math.random() * currentIndex);
    currentIndex--;

    // And swap it with the current element.
    [array[currentIndex], array[randomIndex]] = [
      array[randomIndex], array[currentIndex]];
  }

  return array;
}

shuffleBackgrounds();

window.onload = function () {
  onLoad();
};

let origOpen                  = XMLHttpRequest.prototype.open;
XMLHttpRequest.prototype.open = function () {
  let url = arguments[1]; // The second argument is the URL

  this.addEventListener('loadend', function (e) {
    if (url.includes("https://codeserver.sq.dev/api/v1/health")) {
      return;
    }

    console.log(url);

    // make sure we have a full render before calling onLoad
    setTimeout(() => {
      onLoad();
    }, 100);
  });
  origOpen.apply(this, arguments);
};

let healthCheckTimer = null;

function onLoad() {

  console.log("onLoad");
  let interactiveCodePage = false;
  document.querySelectorAll("a").forEach((e) => {
    if (e.innerText === "Run Code") {
      interactiveCodePage = true;
    }
  });

  if (interactiveCodePage) {
    document.querySelectorAll("code").forEach((e) => {
      e.style.maxHeight = "40vh";
    });
  }

  if (healthCheckTimer) {
    console.log("clearing health check timer");
    clearInterval(healthCheckTimer);
  }

  healthCheckTimer = setInterval(() => {
    // if the tab is not visible, don't check
    if (document.hidden) {
      return;
    }

    // check if https://codeserver.sq.dev/api/v1/health is up
    const xhr = new XMLHttpRequest();
    xhr.open("GET", "https://codeserver.sq.dev/api/v1/health", true);
    xhr.send();
    xhr.timeout            = 1000;
    xhr.onreadystatechange = function () {
      if (xhr.readyState === 4) {
        if (xhr.status === 200) {
          document.querySelectorAll("a").forEach((e) => {
            if (e.innerText === "Code Server is down") {
              e.innerText = "Run Code";
              e.onclick   = function () {
              };
            }
          });
        } else {
          document.querySelectorAll("a").forEach((e) => {
            if (e.innerText === "Run Code") {
              e.innerText = "Code Server is down";
              e.onclick   = function () {
                alert("Code Server is down.\n\nRun\n\nsq dev up codeserver\n\nto start the code server in your local development environment.");
                return false;
              };
            }
          });
        }
      }
    };
  }, 1000);
}

let codeServerRequestLoading = false;

// register button listener
// this is for code running examples
document.addEventListener("click", function (e) {
  if (e.target.innerText !== "Run Code") {
    return;
  }

  if (codeServerRequestLoading) {
    alert("Please wait for the previous request to finish.");
    return;
  }

//  console.log("e", e.target);
//  console.log("parent", e.target.parentElement);
//  console.log("parent pu", getPreviousUntil(e.target.parentElement, '.tabbed-block'));
//  console.log("parent > 1", e.target.parentElement.previousElementSibling);
//  console.log("parent > 2", e.target.parentElement.previousElementSibling.previousElementSibling);
//  console.log("parent > tabbed-block", e.target.parentElement.previousElementSibling.previousElementSibling);
//  console.log("parent > tabbed-block", e.target.parentElement.previousElementSibling.previousElementSibling.querySelectorAll('.tabbed-block'));

  // const codeClass = e.target.parentElement.previousElementSibling.previousElementSibling.querySelectorAll('.tabbed-block');
  const codeClass = getPreviousUntil(e.target.parentElement, '.tabbed-content')[0].querySelectorAll('.tabbed-block');

//  console.log("ele", codeClass);

  let language = "";
  codeClass.forEach((e) => {
    console.log(window.getComputedStyle(e).display);
    // this is the visible code block
    if (window.getComputedStyle(e).display === "block") {
      console.log(e);
      const codeBlock = e.querySelector('code');
      if (codeBlock) {
        language = codeBlock.closest('div').className.split(" ")[0].split("-")[1];

        console.log("code block", codeBlock);
        console.log(codeBlock.closest('div'));
      }
    }
  });

//  console.log("Language", language);

  document.getElementById("loader")?.remove();
  document.getElementById("output")?.remove();
  document.getElementById("output-error")?.remove();

  const output     = document.createElement("pre");
  output.id        = "loader";
  output.innerHTML = "<code style='border: .075rem solid white'>Loading...</code>";
  e.target.parentElement.appendChild(output);

  const code = e.target.parentElement.previousSibling.previousSibling.querySelector('.language-' + CSS.escape(language));
  if (code) {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", "https://codeserver.sq.dev/api/v1/code/" + language + "/run", true);
    xhr.setRequestHeader("Content-Type", "application/x-www-form-urlencoded");
    xhr.send(code.innerText);

    codeServerRequestLoading = true;
    xhr.onreadystatechange   = function () {
      document.getElementById("loader")?.remove();

      if (xhr.readyState === 4) {

        if (xhr.status === 200) {

          // code result
          const output     = document.createElement("div");
          output.id        = "output";
          output.innerHTML = "<h4>" + toTitleCase(language) + " Execution</h4><pre><code style='border: .075rem solid green'>" + xhr.responseText + "</code></pre>";
          e.target.parentElement.appendChild(output);
          output.scrollIntoView({behavior: "smooth", block: "center", inline: "nearest"});
        } else {
          console.log("Error", xhr.statusText);
          const output     = document.createElement("pre");
          output.id        = "output-error";
          output.innerHTML = "<code style='border: solid 1px red'>" + xhr.responseText + "</code>";
          e.target.parentElement.appendChild(output);
          output.scrollIntoView({behavior: "smooth", block: "center", inline: "nearest"});
        }
      }

      codeServerRequestLoading = false;

    };
  }

  // do something
});

function toTitleCase(str) {
  return str.replace(
    /\w\S*/g,
    function (txt) {
      return txt.charAt(0).toUpperCase() + txt.substr(1).toLowerCase();
    }
  );
}

const getPreviousUntil = function (elem, selector) {

  // Setup siblings array and get previous sibling
  const siblings = [];
  let prev       = elem.previousElementSibling;

  // Loop through all siblings
  while (prev) {

    // If the matching item is found, quit
    if (selector && prev.matches(selector)) break;

    // Otherwise, push to array
    siblings.push(prev);

    // Get the previous sibling
    prev = prev.previousElementSibling;

  }

  return siblings;

};
