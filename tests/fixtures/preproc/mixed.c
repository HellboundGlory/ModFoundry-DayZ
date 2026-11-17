// A small lexer input: comments, directives, strings, numbers, an inactive
// branch and the words the scan loop has to survive.
#ifndef FLAG
class PrA
{
	int count = 0x1F;
	string label = "a\"b";
	char junk = 'x';
}
#endif

#ifdef FLAG
void F()
{
	int value = 1e3;
	value = value - 1;
	value += -2;
}
#else
void G() { value = .5; }
#endif

/* block
   comment */
class PrB : PrA
{
	override void Tick()
	{
		if (count > 0) count--;
	}
}
