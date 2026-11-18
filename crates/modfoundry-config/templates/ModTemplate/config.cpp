class CfgPatches
{
    class {{PatchClass}}
    {
        units[] = {};
        weapons[] = {};
        requiredVersion = 0.1;
        requiredAddons[] = { {{RequiredAddons}} };
    };
};

class CfgMods
{
    class {{ModName}}
    {
        type = "mod";
        version = "{{Version}}";
        class defs
        {
{{ScriptModuleDefs}}
        };
    };
};
