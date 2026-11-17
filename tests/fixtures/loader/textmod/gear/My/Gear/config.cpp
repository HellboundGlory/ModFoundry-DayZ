// A hand-written config in the shape a mod ships.
class CfgPatches
{
	class My_Gear
	{
		requiredAddons[] = {"DZ_Data", "My_Core"};
		units[] = {};
	};
};
class CfgMods
{
	class My_Gear_Mod
	{
		type = "mod";
		storageVersion = 2;
		inputs = "My/Gear/data/inputs.xml";
		defines[] = {"MY_GEAR"};
		dependencies[] = {"Game", "World"};
		class defs
		{
			class gameScriptModule
			{
				value = "";
				files[] = {"My/Gear/code/game"};
			};
			class worldScriptModule
			{
				value = "";
				files[] = {"my/gear/CODE/world", "My/Gear/extra"};
			};
		};
	};
};
class CfgVehicles
{
	class Inventory_Base;
	class My_Item: Inventory_Base
	{
		scope = 2; /* spawnable */
		weight = 1.5;
	};
};
