// PDF Epsilon product page: language, the download button for the visitor's system, and the
// current version from GitHub. Works without this script too; it only improves the page.

const REPO = "yusungchang/pdf-epsilon";
const DL = `https://github.com/${REPO}/releases/latest/download/`;

// Korean text for every element with data-t. English is in the HTML itself.
const KO = {
  "nav.features": "기능",
  "nav.download": "다운로드",
  "hero.title": "PDF는 가볍게,<br>중요한 건 그대로.",
  "hero.lead": "PDF가 왜 무거운지 페이지마다 보여 주고, 내 컴퓨터에서 가볍게 줄입니다. 글자 하나 바꾸지 않고요.",
  "hero.download": "다운로드",
  "hero.other": "모든 다운로드",
  "b1.label": "로컬 처리", "b1.sub": "내 파일은 내 컴퓨터에만",
  "b2.label": "결정론적 처리", "b2.sub": "시킨 일만, 정확하게",
  "b3.label": "스트레스 테스트", "b3.sub": "손상 파일 20만 개도 거뜬히",
  "b4.label": "CVE 점검", "b4.sub": "빌드마다, 매주 확인",
  "b5.label": "외부 통신 없음", "b5.sub": "업데이트도 원할 때만",
  "b7.label": "무상 제공", "b7.sub": "공짜. 진짜 있습니다.",
  "features.title": "먼저 이해하고, 그다음 줄입니다",
  "f1.title": "용량은 어디에",
  "f1.text": "파일 전체와 페이지마다, 모든 바이트를 셉니다.",
  "f2.title": "모두 실측",
  "f2.text": "프리셋마다 미리 돌려 봅니다. 보이는 크기가 저장될 크기입니다.",
  "f3.title": "저장 전에 비교",
  "f3.text": "어느 페이지든 끌어서 원본과 작아진 사본을 견줘 봅니다.",
  "f4.title": "중요한 곳은 선명하게",
  "f4.text": "사진은 JPEG으로, 차트와 스크린샷은 무손실로.",
  "f5.title": "가벼워진 글꼴",
  "f5.text": "쓰인 글자만 남깁니다. 15MB 파일이 80KB가 되기도 했습니다.",
  "f6.title": "파일은 집에 머뭅니다",
  "f6.text": "모든 일이 내 컴퓨터 안에서 일어납니다. 업로드도, 가입도, 엿보는 사람도 없습니다.",
  "sec.title": "가장 고약한 PDF로 시험했습니다",
  "sec.sub": "PDF는 여는 프로그램을 멈추거나 망가뜨리거나 속이도록 만들 수도 있습니다. PDF Epsilon은 출시 전에 이런 시험을 거쳤습니다.",
  "sec.n1": "무작위로 손상시킨 PDF,<br>충돌·멈춤 0건",
  "sec.n2": "qpdf 테스트 모음의<br>깨지고 악의적인 파일",
  "sec.n3": "압축한 사본의 글자가<br>원본과 똑같음",
  "sec.n4": "압축 폭탄을<br>450MB 안에서 멈춤",
  "sec.c1": "손상된 부분은 짐작으로 고치지 않습니다. 그런 파일은 그대로 둡니다.",
  "sec.c2": "페이지 그림은 메모리가 제한된 별도 프로세스에서 그립니다. 나쁜 파일이 앱을 멈추지 못합니다.",
  "sec.c3": "빌드할 때마다 모든 라이브러리를 공개된 취약점 목록과 대조합니다.",
  "sec.more": "악의적인 파일을 다루는 방법 자세히 보기",
  "results.title": "실제 파일로 잰 결과",
  "results.sub": "균형 프리셋, 텍스트와 페이지는 그대로.",
  "r1": "한글 글꼴이 든 신문 기사<br>3.4MB → 313KB",
  "r2": "그림이 많은 연구 논문<br>3.1MB → 773KB",
  "r3": "컨퍼런스 발표 자료<br>3.3MB → 1.1MB",
  "r4": "49쪽 설문 보고서<br>3.0MB → 2.2MB",
  "compare.title": "저장하기 전에 직접 보세요",
  "compare.text": "지금 모습과 저장될 모습을 나란히 봅니다. 비교한 그대로 저장됩니다.",
  "download.title": "다운로드",
  "download.sub": "macOS, Windows, Linux용 무료 앱입니다.",
  "dl.macArm": "Apple 실리콘 (M1 이후)",
  "dl.macIntel": "Intel",
  "dl.macWhich": "Apple 메뉴 → 이 Mac에 관하여: ‘칩’이면 Apple 실리콘, ‘프로세서’면 Intel.",
  "dl.winSetup": "설치 프로그램 (.exe)",
  "dl.winMsi": "MSI 패키지",
  "dl.winNote": "Windows 10, 11 (64비트).",
  "dl.deb": "Debian, Ubuntu, Mint (.deb)",
  "dl.rpm": "Fedora (.rpm)",
  "first.title": "처음 실행할 때",
  "first.mac": "응용 프로그램 폴더로 옮긴 뒤 여세요. 확인할 수 없다는 안내가 뜨면 시스템 설정 → 개인정보 보호 및 보안 → ‘그래도 열기’. 처음 한 번만입니다.",
  "first.win": "‘Windows의 PC 보호’ 창이 뜨면 ‘추가 정보’ → ‘실행’을 누르세요.",
  "first.linux": ".deb는 받은 폴더에서 <code>sudo apt install ./PDF-Epsilon-Linux.deb</code>를 실행하세요. AppImage는 <code>chmod +x PDF-Epsilon-Linux.AppImage</code>를 실행한 다음 여세요.",
  "first.mac1": ".dmg를 열고 PDF Epsilon을 응용 프로그램 폴더로 끌어 놓으세요.",
  "first.mac2": "앱을 여세요. 확인할 수 없다는 창이 뜨면 ‘완료’를 누르세요.",
  "first.mac3": "시스템 설정 → 개인정보 보호 및 보안에서 아래로 내려 <b>그래도 열기</b>를 누르세요. 처음 한 번만입니다.",
  "first.win1": "설치 프로그램을 실행하세요.",
  "first.win2": "‘Windows의 PC 보호’ 창이 뜨면 <b>추가 정보</b> → <b>실행</b>을 누르세요.",
  "first.linux1": ".deb: <code>sudo apt install ./PDF-Epsilon-Linux.deb</code>",
  "first.linux2": "AppImage: <code>chmod +x PDF-Epsilon-Linux.AppImage</code> 실행 후 여세요.",
  "after.title": "다운로드가 시작됐습니다",
  "after.text": "아직 유료 인증서로 서명하지 않아서, 처음 열 때만 이렇게 해 주세요.",
  "after.close": "확인",
  "download.all": "모든 버전과 릴리스 노트",
  "footer.text": "PDF는 집 밖으로 나가지 않습니다.",
  "footer.source": "GitHub의 소스 코드",
  "footer.bug": "버그 신고",
  "footer.author": "만든 사람",
};

const english = new Map();
document.querySelectorAll("[data-t]").forEach((e) => english.set(e, e.innerHTML));

function setLanguage(lang) {
  document.documentElement.lang = lang;
  document.querySelectorAll("[data-t]").forEach((e) => {
    e.innerHTML = lang === "ko" && KO[e.dataset.t] ? KO[e.dataset.t] : english.get(e);
  });
  document.getElementById("lang").textContent = lang === "ko" ? "EN" : "한국어";
  document.getElementById("shot-report").src = lang === "ko" ? "shots/report-ko.png" : "shots/report.png";
  document.getElementById("shot-compare").src = lang === "ko" ? "shots/compare-ko.png" : "shots/compare.png";
  try { localStorage.setItem("lang", lang); } catch { /* storage unavailable */ }
  showPlatform();
  showVersion();
}

// The visitor's system, from the browser. Apple silicon and Intel Macs cannot be told apart
// reliably, so Macs get the Apple silicon button and the Intel one next to it.
function platform() {
  const p = (navigator.userAgentData?.platform || navigator.platform || navigator.userAgent).toLowerCase();
  if (p.includes("win")) return "windows";
  if (p.includes("mac")) return "mac";
  if (p.includes("linux") && !/android/i.test(navigator.userAgent)) return "linux";
  return null;
}

function showPlatform() {
  const ko = document.documentElement.lang === "ko";
  const os = platform();
  const primary = document.getElementById("primary");
  const label = document.getElementById("primary-label");
  const choices = {
    mac: [DL + "PDF-Epsilon-macOS-AppleSilicon.dmg", ko ? "macOS용 다운로드" : "Download for macOS"],
    windows: [DL + "PDF-Epsilon-Windows-Setup.exe", ko ? "Windows용 다운로드" : "Download for Windows"],
    linux: [DL + "PDF-Epsilon-Linux.deb", ko ? "Linux용 다운로드 (.deb)" : "Download for Linux (.deb)"],
  };
  document.querySelectorAll(".platforms article").forEach((a) => a.classList.toggle("mine", a.id === os));
  if (os && choices[os]) {
    primary.href = choices[os][0];
    label.textContent = choices[os][1];
  }
}

// The newest published release, if there is one yet.
let release = null;
async function showVersion() {
  const el = document.getElementById("version");
  try {
    release = release || (await (await fetch(`https://api.github.com/repos/${REPO}/releases/latest`)).json());
    if (!release.tag_name) return;
    const ko = document.documentElement.lang === "ko";
    const date = new Date(release.published_at).toLocaleDateString(ko ? "ko-KR" : "en-US", { year: "numeric", month: "long", day: "numeric" });
    el.textContent = ko
      ? `버전 ${release.tag_name.replace(/^v/, "")} · ${date}`
      : `Version ${release.tag_name.replace(/^v/, "")} · ${date}`;
    el.hidden = false;
  } catch { /* offline or rate limited: the page works without it */ }
}

// Any download link: let the download start, then show the first-launch steps for that system.
// The steps are copied from the download card, so they are already in the visitor's language.
// One listener for the page, because the top button only gets its download link after load.
document.addEventListener("click", (e) => {
  const a = e.target.closest?.('a[href*="/releases/latest/download/"]');
  if (!a) return;
  const n = a.href.split("/").pop();
  const os = n.includes("macOS") ? "mac" : n.includes("Windows") ? "windows" : "linux";
  const steps = document.querySelector(`.steps[data-os="${os}"] ol`);
  const dialog = document.getElementById("after");
  if (!steps || !dialog.showModal) return;
  document.getElementById("after-steps").replaceChildren(steps.cloneNode(true));
  dialog.showModal();
});
document.getElementById("after").addEventListener("click", (e) => {
  if (e.target === e.currentTarget) e.currentTarget.close();
});

document.getElementById("lang").addEventListener("click", () => setLanguage(document.documentElement.lang === "ko" ? "en" : "ko"));

// ?lang=ko or ?lang=en in the link wins, then the visitor's last choice, then the browser.
let initial = new URLSearchParams(location.search).get("lang");
if (initial !== "ko" && initial !== "en") {
  try { initial = localStorage.getItem("lang") || ""; } catch { initial = ""; }
}
if (!initial) initial = (navigator.languages || [navigator.language]).some((l) => /^ko/i.test(l)) ? "ko" : "en";
setLanguage(initial);
