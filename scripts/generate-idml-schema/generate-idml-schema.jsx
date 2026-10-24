#target "indesign"

// Run in Adobe InDesign (ExtendScript), keeping it in scripts/generate-idml-schema/.
// Schemas are written at the repository root in schemas/<app.version>/.
(function () {
    var generatorVersion = "0.1.0";
    var scriptFile = File($.fileName);
    if (scriptFile.alias) {
        scriptFile = scriptFile.resolve();
    }
    var projectFolder = scriptFile.parent.parent.parent;
    var indesignVersion = String(app.version);
    var versionFolder = Folder(projectFolder.fsName + "/schemas/" +
        indesignVersion.replace(/[^A-Za-z0-9._-]/g, "_"));

    // Keep previous snapshots intact, including an incomplete previous run.
    if (versionFolder.exists) {
        throw Error("Schema destination already exists: " + versionFolder.fsName +
            "\nMove it aside before generating this version again.");
    }

    function ensureFolder(folder) {
        if (!folder.exists) {
            ensureFolder(folder.parent);
            if (!folder.create()) {
                throw Error("Cannot create folder: " + folder.fsName + "\n" + folder.error);
            }
        }
        return folder;
    }

    function collectSchemas(folder, prefix) {
        var entries = folder.getFiles();
        var paths = [];
        for (var i = 0; i < entries.length; i++) {
            var entry = entries[i];
            var path = prefix + entry.name;
            if (entry instanceof Folder) {
                paths = paths.concat(collectSchemas(entry, path + "/"));
            } else if (/\.rnc$/i.test(entry.name)) {
                if (entry.length === 0) {
                    throw Error("InDesign generated an empty schema: " + entry.fsName);
                }
                paths.push(path);
            }
        }
        return paths.sort();
    }

    function requireSchema(folder, name) {
        var file = File(folder.fsName + "/" + name);
        if (!file.exists || file.length === 0) {
            throw Error("InDesign did not generate the expected schema: " + file.fsName);
        }
    }

    // ExtendScript does not consistently provide JSON.stringify.
    function quote(value) {
        return '"' + String(value).replace(/[\\"\u0000-\u001f]/g, function (character) {
            return "\\u" + ("0000" + character.charCodeAt(0).toString(16)).slice(-4);
        }) + '"';
    }

    function quoteArray(values) {
        var quoted = [];
        for (var i = 0; i < values.length; i++) {
            quoted.push(quote(values[i]));
        }
        return "[" + quoted.join(", ") + "]";
    }

    function writeManifest(file, content) {
        file.encoding = "UTF-8";
        file.lineFeed = "Unix";
        if (!file.open("w")) {
            throw Error("Cannot open manifest: " + file.fsName + "\n" + file.error);
        }
        var written = file.write(content);
        var closed = file.close();
        if (!written || !closed) {
            throw Error("Cannot write manifest: " + file.fsName + "\n" + file.error);
        }
    }

    var packageFolder = ensureFolder(Folder(versionFolder.fsName + "/package"));
    var singleFolder = ensureFolder(Folder(versionFolder.fsName + "/single"));

    // Adobe's workaround: true = IDML package, false = consolidated schema.
    app.generateIDMLSchema(packageFolder, true);
    app.generateIDMLSchema(singleFolder, false);

    requireSchema(packageFolder, "designmap.rnc");
    requireSchema(packageFolder, "datatype.rnc");
    requireSchema(singleFolder, "IDMarkupLanguage.rnc");
    requireSchema(singleFolder, "datatype.rnc");
    var packageFiles = collectSchemas(packageFolder, "");
    var singleFiles = collectSchemas(singleFolder, "");

    // Written only after both exports pass the file checks.
    writeManifest(File(versionFolder.fsName + "/manifest.json"), [
        "{",
        '  "generatorVersion": ' + quote(generatorVersion) + ",",
        '  "indesignVersion": ' + quote(indesignVersion) + ",",
        '  "indesignLocale": ' + quote(app.locale) + ",",
        '  "format": "RELAX NG Compact",',
        '  "packageFiles": ' + quoteArray(packageFiles) + ",",
        '  "singleFiles": ' + quoteArray(singleFiles),
        "}",
        ""
    ].join("\n"));

    $.writeln("Generated IDML schemas: " + versionFolder.fsName);
    return versionFolder.fsName;
}());
