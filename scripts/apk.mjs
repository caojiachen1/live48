import { execSync } from "child_process";
import { existsSync, readdirSync } from "fs";
import { resolve, dirname, join } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const projectRoot = resolve(__dirname, "..");

// 获取 Android SDK 路径
const androidHome = process.env.ANDROID_HOME;
if (!androidHome) {
  console.error("错误: 未设置 ANDROID_HOME 环境变量");
  process.exit(1);
}

// 自动查找 apksigner
function findApksigner() {
  const buildToolsDir = join(androidHome, "build-tools");
  if (!existsSync(buildToolsDir)) {
    console.error(`错误: 未找到 build-tools 目录: ${buildToolsDir}`);
    process.exit(1);
  }

  const versions = readdirSync(buildToolsDir).sort().reverse();
  for (const version of versions) {
    const apksignerPath = join(buildToolsDir, version, "apksigner.bat");
    const apksignerJar = join(buildToolsDir, version, "lib", "apksigner.jar");
    if (existsSync(apksignerPath)) {
      return { type: "bat", path: apksignerPath };
    }
    if (existsSync(apksignerJar)) {
      return { type: "jar", path: apksignerJar };
    }
  }

  console.error("错误: 未找到 apksigner");
  process.exit(1);
}

const apksigner = findApksigner();

// 构建前端
console.log("[1/3] 构建前端...");
execSync("npm run build", { cwd: projectRoot, stdio: "inherit" });

// 构建 Android APK
console.log("[2/3] 构建 Android APK...");
execSync("npm run tauri -- android build", { cwd: projectRoot, stdio: "inherit" });

// 签名 APK
const unsignedApk = resolve(
  projectRoot,
  "src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk"
);
const keystore = resolve(projectRoot, "android.keystore");
const signedApk = resolve(projectRoot, "live48.apk");

if (!existsSync(unsignedApk)) {
  console.error(`错误: 未找到 APK 文件: ${unsignedApk}`);
  process.exit(1);
}

if (!existsSync(keystore)) {
  console.error(`错误: 未找到密钥库: ${keystore}`);
  process.exit(1);
}

console.log("[3/3] 签名 APK...");
if (apksigner.type === "bat") {
  execSync(
    `"${apksigner.path}" sign --ks "${keystore}" --ks-pass pass:android --key-pass pass:android --out "${signedApk}" "${unsignedApk}"`,
    { stdio: "inherit" }
  );
} else {
  execSync(
    `java -jar "${apksigner.path}" sign --ks "${keystore}" --ks-pass pass:android --key-pass pass:android --out "${signedApk}" "${unsignedApk}"`,
    { stdio: "inherit" }
  );
}
console.log(`\n✅ 打包完成: ${signedApk}`);