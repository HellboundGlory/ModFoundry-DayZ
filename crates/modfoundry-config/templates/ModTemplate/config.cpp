class CfgPatches
{
    class {{ModName}}_Scripts
    {
        units[] = {};
        weapons[] = {};
        requiredVersion = 0.1;
        requiredAddons[] = { "DZ_Data" };
    };
};

class CfgMods
{
    class {{ModName}}
    {
        type = "mod";
        class defs
        {
{{ScriptModuleDefs}}
        };
    };
};
