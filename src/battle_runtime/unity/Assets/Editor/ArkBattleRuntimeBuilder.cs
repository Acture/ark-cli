// Place in Assets/Editor of the disposable Unity 2021.3.39f1 project.
// Editor-only code: no new runtime MonoBehaviour is required by the empty player scene.
using System;
using System.IO;
using UnityEditor;
using UnityEditor.Build.Reporting;
using UnityEditor.SceneManagement;
using UnityEngine;

public static class ArkBattleRuntimeBuilder
{
	public static void Build()
	{
		if (Application.unityVersion != "2021.3.39f1")
			throw new InvalidOperationException("This compatibility experiment requires 2021.3.39f1.");

		string pluginPath = "Assets/Plugins/x86_64/ArkBattleProbe.dll";
		AssetDatabase.Refresh(ImportAssetOptions.ForceSynchronousImport);
		PluginImporter importer = AssetImporter.GetAtPath(pluginPath) as PluginImporter;
		if (importer == null || !importer.isNativePlugin)
			throw new InvalidOperationException("The compiled battle probe is missing.");
		importer.SetCompatibleWithAnyPlatform(false);
		importer.SetCompatibleWithEditor(false);
		importer.SetCompatibleWithPlatform(BuildTarget.StandaloneWindows64, true);
		importer.SetPlatformData(BuildTarget.StandaloneWindows64, "CPU", "x86_64");
		importer.isPreloaded = true;
		importer.SaveAndReimport();

		string projectRoot = Path.GetFullPath(Path.Combine(Application.dataPath, ".."));
		string output = Path.Combine(projectRoot, "_PlayerOutput",
			DateTime.UtcNow.ToString("yyyyMMddTHHmmssfffffffZ"));
		if (Directory.Exists(output))
			throw new InvalidOperationException("Preserve the existing build output.");
		Directory.CreateDirectory(output);

		PlayerSettings.companyName = "ArkCli";
		PlayerSettings.productName = "ArkBattleRuntime";
		PlayerSettings.runInBackground = true;
		PlayerSettings.stripEngineCode = false;
		PlayerSettings.SetScriptingBackend(BuildTargetGroup.Standalone,
			ScriptingImplementation.IL2CPP);
		PlayerSettings.SetIl2CppCompilerConfiguration(BuildTargetGroup.Standalone,
			Il2CppCompilerConfiguration.Release);
		PlayerSettings.SetManagedStrippingLevel(BuildTargetGroup.Standalone,
			ManagedStrippingLevel.Low);

		string scenePath = "Assets/ArkBattleRuntimeEmpty.unity";
		UnityEngine.SceneManagement.Scene scene = EditorSceneManager.NewScene(
			NewSceneSetup.EmptyScene, NewSceneMode.Single);
		GameObject root = new GameObject("RuntimeRoot");
		if (root.GetComponents<Component>().Length != 1)
			throw new InvalidOperationException("The empty scene must contain only a Transform.");
		if (!EditorSceneManager.SaveScene(scene, scenePath))
			throw new InvalidOperationException("The empty scene could not be saved.");

		BuildPlayerOptions options = new BuildPlayerOptions
		{
			scenes = new[] { scenePath },
			locationPathName = Path.Combine(output, "ArkBattleRuntime.exe"),
			target = BuildTarget.StandaloneWindows64,
			options = BuildOptions.StrictMode
		};
		BuildReport report = BuildPipeline.BuildPlayer(options);
		BuildEvidence evidence = new BuildEvidence
		{
			unityVersion = Application.unityVersion,
			result = report.summary.result.ToString(),
			outputPath = report.summary.outputPath,
			pluginPreloaded = importer.isPreloaded,
			errors = report.summary.totalErrors,
			warnings = report.summary.totalWarnings
		};
		File.WriteAllText(Path.Combine(output, "player-build-result.json"),
			JsonUtility.ToJson(evidence, true));
		if (report.summary.result != BuildResult.Succeeded)
			throw new InvalidOperationException("Clean Player build failed: " + report.summary.result);
		Debug.Log("ARK_BATTLE_RUNTIME_BUILT " + report.summary.outputPath);
	}

	[Serializable]
	private sealed class BuildEvidence
	{
		public string unityVersion;
		public string result;
		public string outputPath;
		public bool pluginPreloaded;
		public int errors;
		public int warnings;
		public bool originalRuntimeLoaded = false;
		public bool battleExecuted = false;
	}
}
