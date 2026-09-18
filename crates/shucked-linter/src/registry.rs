use crate::Severity;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Correctness,
    Style,
    Performance,
    Portability,
    Security,
}

impl Category {
    pub fn from_prefix(prefix: &str) -> Option<Self> {
        match prefix {
            "C" => Some(Self::Correctness),
            "S" => Some(Self::Style),
            "P" => Some(Self::Performance),
            "X" => Some(Self::Portability),
            "K" => Some(Self::Security),
            _ => None,
        }
    }
}

macro_rules! declare_rules {
    (@sub $name:ident) => {
        ()
    };
    (@count $($name:ident),+ $(,)?) => {
        <[()]>::len(&[$(declare_rules!(@sub $name)),+])
    };
    ($(
        ($code:literal, $category:expr, $severity:expr, $name:ident),
    )+) => {
        /// Identifier for a registered lint rule.
        ///
        /// New rules are added routinely. Downstream matches must include a fallback arm; use
        /// [`Rule::code`] for a stable persisted identity and [`Rule::iter`] to discover rules.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        #[repr(u16)]
        pub enum Rule {
            $($name,)*
        }

        pub const ALL_RULES: [Rule; declare_rules!(@count $($name),+)] = [
            $(Rule::$name,)*
        ];

        impl Rule {
            pub const COUNT: usize = ALL_RULES.len();

            pub fn iter() -> impl ExactSizeIterator<Item = Self> + DoubleEndedIterator + Clone {
                ALL_RULES.into_iter()
            }

            pub const fn code(self) -> &'static str {
                match self {
                    $(Self::$name => $code,)*
                }
            }

            pub const fn category(self) -> Category {
                match self {
                    $(Self::$name => $category,)*
                }
            }

            pub const fn default_severity(self) -> Severity {
                match self {
                    $(Self::$name => $severity,)*
                }
            }
        }

        fn canonical_code_to_rule(code: &str) -> Option<Rule> {
            match code {
                $($code => Some(Rule::$name),)*
                _ => None,
            }
        }
    };
}

declare_rules! {
    ("C001", Category::Correctness, Severity::Warning, UnusedAssignment),
    ("C002", Category::Correctness, Severity::Warning, DynamicSourcePath),
    ("C003", Category::Correctness, Severity::Warning, UntrackedSourceFile),
    ("C004", Category::Correctness, Severity::Warning, UncheckedDirectoryChange),
    ("C005", Category::Correctness, Severity::Warning, SingleQuotedLiteral),
    ("C006", Category::Correctness, Severity::Error, UndefinedVariable),
    ("C007", Category::Correctness, Severity::Warning, FindOutputToXargs),
    ("C008", Category::Correctness, Severity::Warning, TrapStringExpansion),
    ("C009", Category::Correctness, Severity::Warning, QuotedBashRegex),
    ("C010", Category::Correctness, Severity::Warning, ChainedTestBranches),
    ("C011", Category::Correctness, Severity::Warning, LineOrientedInput),
    ("C012", Category::Correctness, Severity::Warning, LeadingGlobArgument),
    ("C013", Category::Correctness, Severity::Warning, FindOutputLoop),
    ("C014", Category::Correctness, Severity::Error, LocalTopLevel),
    ("C015", Category::Correctness, Severity::Warning, SudoRedirectionOrder),
    ("C016", Category::Correctness, Severity::Error, StrayClosingKeyword),
    ("C017", Category::Correctness, Severity::Warning, ConstantComparisonTest),
    ("C018", Category::Correctness, Severity::Error, LoopControlOutsideLoop),
    ("C019", Category::Correctness, Severity::Warning, LiteralUnaryStringTest),
    ("C020", Category::Correctness, Severity::Warning, TruthyLiteralTest),
    ("C021", Category::Correctness, Severity::Warning, ConstantCaseSubject),
    ("C022", Category::Correctness, Severity::Error, EmptyTest),
    ("C023", Category::Correctness, Severity::Error, LeadingZeroArithmetic),
    ("C024", Category::Correctness, Severity::Warning, AssignmentSpacing),
    ("C030", Category::Correctness, Severity::Error, MissingBracketSpace),
    ("C031", Category::Correctness, Severity::Error, MissingSpaceBeforeBracketClose),
    ("C032", Category::Correctness, Severity::Error, JammedTestBracket),
    ("C033", Category::Correctness, Severity::Error, IndentedHeredocClose),
    ("C025", Category::Correctness, Severity::Warning, PositionalTenBraces),
    ("C027", Category::Correctness, Severity::Warning, BareDoneWord),
    ("C034", Category::Correctness, Severity::Error, UnterminatedIf),
    ("C035", Category::Correctness, Severity::Error, MissingFi),
    ("C036", Category::Correctness, Severity::Error, BrokenTestEnd),
    ("C037", Category::Correctness, Severity::Error, BrokenTestParse),
    ("C038", Category::Correctness, Severity::Error, ElseIf),
    ("C039", Category::Correctness, Severity::Warning, OpenDoubleQuote),
    ("C040", Category::Correctness, Severity::Error, LinebreakInTest),
    ("C041", Category::Correctness, Severity::Error, CStyleComment),
    ("C042", Category::Correctness, Severity::Warning, CPrototypeFragment),
    ("C043", Category::Correctness, Severity::Warning, BadRedirectionFdOrder),
    ("C044", Category::Correctness, Severity::Warning, BareGlobCommandPath),
    ("C045", Category::Correctness, Severity::Warning, DiffMarkerLine),
    ("C046", Category::Correctness, Severity::Warning, PipeToKill),
    ("C047", Category::Correctness, Severity::Error, InvalidExitStatus),
    ("C048", Category::Correctness, Severity::Warning, CasePatternVar),
    ("C049", Category::Correctness, Severity::Warning, TautologyChain),
    ("C050", Category::Correctness, Severity::Warning, ArithmeticRedirectionTarget),
    ("C051", Category::Correctness, Severity::Error, DuplicateRedirect),
    ("C052", Category::Correctness, Severity::Error, AssignSpecialZero),
    ("C053", Category::Correctness, Severity::Warning, SpaceyAssign),
    ("C054", Category::Correctness, Severity::Warning, BareSlashMarker),
    ("C055", Category::Correctness, Severity::Warning, PatternWithVariable),
    ("C056", Category::Correctness, Severity::Warning, StatusCaptureAfterBranchTest),
    ("C057", Category::Correctness, Severity::Warning, SubstWithRedirect),
    ("C058", Category::Correctness, Severity::Warning, SubstWithRedirectErr),
    ("C059", Category::Correctness, Severity::Warning, RedirectToCommandName),
    ("C060", Category::Correctness, Severity::Warning, NonAbsoluteShebang),
    ("C061", Category::Correctness, Severity::Warning, TemplateBraceInCommand),
    ("C062", Category::Correctness, Severity::Warning, NestedParameterExpansion),
    ("C063", Category::Correctness, Severity::Warning, OverwrittenFunction),
    ("C064", Category::Correctness, Severity::Warning, IfMissingThen),
    ("C065", Category::Correctness, Severity::Warning, ElseWithoutThen),
    ("C066", Category::Correctness, Severity::Warning, MissingSemicolonBeforeBrace),
    ("C067", Category::Correctness, Severity::Warning, EmptyFunctionBody),
    ("C068", Category::Correctness, Severity::Warning, BareClosingBrace),
    ("C069", Category::Correctness, Severity::Warning, BackslashBeforeClosingBacktick),
    ("C070", Category::Correctness, Severity::Warning, PositionalParamAsOperator),
    ("C071", Category::Correctness, Severity::Warning, DoubleParenGrouping),
    ("C072", Category::Correctness, Severity::Warning, UnicodeQuoteInString),
    ("C073", Category::Correctness, Severity::Warning, IndentedShebang),
    ("C074", Category::Correctness, Severity::Warning, SpaceAfterHashBang),
    ("C075", Category::Correctness, Severity::Warning, ShebangNotOnFirstLine),
    (
        "C076",
        Category::Correctness,
        Severity::Warning,
        CommentedContinuationLine
    ),
    ("C077", Category::Correctness, Severity::Warning, SubshellInArithmetic),
    ("C078", Category::Correctness, Severity::Warning, UnquotedGlobsInFind),
    ("C080", Category::Correctness, Severity::Warning, GlobInGrepPattern),
    ("C081", Category::Correctness, Severity::Warning, GlobInStringComparison),
    ("C083", Category::Correctness, Severity::Warning, GlobInFindSubstitution),
    ("C084", Category::Correctness, Severity::Warning, UnquotedGrepRegex),
    (
        "C085",
        Category::Correctness,
        Severity::Warning,
        StderrBeforeStdoutRedirect
    ),
    (
        "C094",
        Category::Correctness,
        Severity::Warning,
        RedirectClobbersInput
    ),
    (
        "C082",
        Category::Correctness,
        Severity::Warning,
        EscapedNegationInTest
    ),
    (
        "C086",
        Category::Correctness,
        Severity::Warning,
        GreaterThanInTest
    ),
    (
        "C087",
        Category::Correctness,
        Severity::Warning,
        StringComparisonForVersion
    ),
    (
        "C088",
        Category::Correctness,
        Severity::Warning,
        MixedAndOrInCondition
    ),
    (
        "C089",
        Category::Correctness,
        Severity::Warning,
        QuotedCommandInTest
    ),
    (
        "C090",
        Category::Correctness,
        Severity::Warning,
        GlobInTestComparison
    ),
    (
        "C091",
        Category::Correctness,
        Severity::Warning,
        TildeInStringComparison
    ),
    ("C092", Category::Correctness, Severity::Warning, IfDollarCommand),
    (
        "C093",
        Category::Correctness,
        Severity::Warning,
        BacktickInCommandPosition
    ),
    (
        "C095",
        Category::Correctness,
        Severity::Warning,
        AssignmentLooksLikeComparison
    ),
    ("C096", Category::Correctness, Severity::Warning, SuspiciousBracketGlob),
    (
        "C097",
        Category::Correctness,
        Severity::Error,
        FunctionCalledWithoutArgs
    ),
    (
        "C098",
        Category::Correctness,
        Severity::Warning,
        SetFlagsWithoutDashes
    ),
    (
        "C099",
        Category::Correctness,
        Severity::Warning,
        QuotedArraySlice
    ),
    (
        "C100",
        Category::Correctness,
        Severity::Warning,
        QuotedBashSource
    ),
    (
        "C101",
        Category::Correctness,
        Severity::Warning,
        IfsSetToLiteralBackslashN
    ),
    (
        "C102",
        Category::Correctness,
        Severity::Warning,
        GlobInTestDirectory
    ),
    (
        "C103",
        Category::Correctness,
        Severity::Warning,
        FindOrWithoutGrouping
    ),
    (
        "C104",
        Category::Correctness,
        Severity::Warning,
        NonShellSyntaxInScript
    ),
    (
        "C105",
        Category::Correctness,
        Severity::Warning,
        ExportWithPositionalParams
    ),
    (
        "C106",
        Category::Correctness,
        Severity::Warning,
        AppendToArrayAsString
    ),
    (
        "C107",
        Category::Correctness,
        Severity::Warning,
        DollarQuestionAfterCommand
    ),
    (
        "C108",
        Category::Correctness,
        Severity::Warning,
        UnsetAssociativeArrayElement
    ),
    (
        "C109",
        Category::Correctness,
        Severity::Warning,
        MapfileProcessSubstitution
    ),
    (
        "C111",
        Category::Correctness,
        Severity::Warning,
        AtSignInStringCompare
    ),
    (
        "C112",
        Category::Correctness,
        Severity::Warning,
        ArraySliceInComparison
    ),
    (
        "C114",
        Category::Correctness,
        Severity::Warning,
        GlobWithExpansionInLoop
    ),
    (
        "C116",
        Category::Correctness,
        Severity::Warning,
        AssignmentToNumericVariable
    ),
    (
        "C117",
        Category::Correctness,
        Severity::Warning,
        PlusPrefixInAssignment
    ),
    (
        "C118",
        Category::Correctness,
        Severity::Warning,
        MalformedArithmeticInCondition
    ),
    (
        "C119",
        Category::Correctness,
        Severity::Warning,
        RedirectBeforePipe
    ),
    (
        "C120",
        Category::Correctness,
        Severity::Warning,
        ExprSubstrInTest
    ),
    (
        "C121",
        Category::Correctness,
        Severity::Warning,
        StringComparedWithEq
    ),
    (
        "C122",
        Category::Correctness,
        Severity::Warning,
        AFlagInDoubleBracket
    ),
    (
        "C123",
        Category::Correctness,
        Severity::Error,
        FunctionReferencesUnsetParam
    ),
    ("C124", Category::Correctness, Severity::Warning, UnreachableAfterExit),
    ("C127", Category::Correctness, Severity::Warning, UnusedHeredoc),
    (
        "C125",
        Category::Correctness,
        Severity::Warning,
        UncheckedDirectoryChangeInFunction
    ),
    (
        "C126",
        Category::Correctness,
        Severity::Error,
        ContinueOutsideLoopInFunction
    ),
    (
        "C128",
        Category::Correctness,
        Severity::Warning,
        CaseGlobReachability
    ),
    (
        "C129",
        Category::Correctness,
        Severity::Warning,
        CaseDefaultBeforeGlob
    ),
    (
        "C130",
        Category::Correctness,
        Severity::Warning,
        AppendWithEscapedQuotes
    ),
    (
        "C131",
        Category::Correctness,
        Severity::Warning,
        VariableAsCommandName
    ),
    (
        "C132",
        Category::Correctness,
        Severity::Warning,
        EnvPrefixExpansionOnly
    ),
    (
        "C133",
        Category::Correctness,
        Severity::Warning,
        ArrayToStringConversion
    ),
    (
        "C134",
        Category::Correctness,
        Severity::Warning,
        GetoptsOptionNotInCase
    ),
    (
        "C135",
        Category::Correctness,
        Severity::Warning,
        CaseArmNotInGetopts
    ),
    (
        "C136",
        Category::Correctness,
        Severity::Warning,
        LocalCrossReference
    ),
    (
        "C137",
        Category::Correctness,
        Severity::Warning,
        UnicodeSingleQuoteInSingleQuotes
    ),
    ("C138", Category::Correctness, Severity::Warning, HeredocMissingEnd),
    ("C139", Category::Correctness, Severity::Warning, SpacedAssignment),
    ("C140", Category::Correctness, Severity::Warning, BadVarName),
    ("C141", Category::Correctness, Severity::Error, LoopWithoutEnd),
    (
        "C142",
        Category::Correctness,
        Severity::Error,
        MissingDoneInForLoop
    ),
    ("C143", Category::Correctness, Severity::Error, DanglingElse),
    (
        "C144",
        Category::Correctness,
        Severity::Warning,
        HeredocCloserNotAlone
    ),
    ("C145", Category::Correctness, Severity::Warning, MisquotedHeredocClose),
    ("C146", Category::Correctness, Severity::Error, UntilMissingDo),
    ("C148", Category::Correctness, Severity::Warning, BrokenAssocKey),
    (
        "C150",
        Category::Correctness,
        Severity::Warning,
        SubshellLocalAssignment
    ),
    ("C151", Category::Correctness, Severity::Warning, CommaArrayElements),
    ("C155", Category::Correctness, Severity::Warning, SubshellSideEffect),
    (
        "C156",
        Category::Correctness,
        Severity::Warning,
        PossibleVariableMisspelling
    ),
    ("C157", Category::Correctness, Severity::Error, IfBracketGlued),
    (
        "C158",
        Category::Correctness,
        Severity::Warning,
        ImplicitGlobalInFunction
    ),
    ("C159", Category::Correctness, Severity::Warning, MutableGlobal),
    (
        "C160",
        Category::Correctness,
        Severity::Warning,
        UnanchoredSourcePath
    ),
    (
        "C161",
        Category::Correctness,
        Severity::Error,
        FunctionCalledBeforeDefined
    ),
    ("P001", Category::Performance, Severity::Warning, ExprArithmetic),
    ("P002", Category::Performance, Severity::Warning, GrepCountPipeline),
    ("P003", Category::Performance, Severity::Warning, SingleTestSubshell),
    ("P004", Category::Performance, Severity::Warning, SubshellTestGroup),
    ("X001", Category::Portability, Severity::Warning, DoubleBracketInSh),
    ("X002", Category::Portability, Severity::Warning, TestEqualityOperator),
    ("X003", Category::Portability, Severity::Warning, LocalVariableInSh),
    ("X004", Category::Portability, Severity::Warning, FunctionKeyword),
    ("X005", Category::Portability, Severity::Warning, BashCaseFallthrough),
    ("X006", Category::Portability, Severity::Warning, ProcessSubstitution),
    ("X007", Category::Portability, Severity::Warning, AnsiCQuoting),
    ("X010", Category::Portability, Severity::Warning, BraceExpansion),
    ("X011", Category::Portability, Severity::Warning, HereString),
    ("X008", Category::Portability, Severity::Warning, StandaloneArithmetic),
    ("X009", Category::Portability, Severity::Warning, SelectLoop),
    ("X014", Category::Portability, Severity::Warning, Coproc),
    ("X012", Category::Portability, Severity::Warning, AmpersandRedirection),
    ("X013", Category::Portability, Severity::Warning, ArrayAssignment),
    ("X015", Category::Portability, Severity::Warning, LetCommand),
    ("X016", Category::Portability, Severity::Warning, DeclareCommand),
    ("X017", Category::Portability, Severity::Warning, TrapErr),
    ("X018", Category::Portability, Severity::Warning, IndirectExpansion),
    ("X019", Category::Portability, Severity::Warning, ArrayReference),
    ("X020", Category::Portability, Severity::Warning, BraceFdRedirection),
    ("X021", Category::Portability, Severity::Warning, PipefailOption),
    ("X022", Category::Portability, Severity::Warning, WaitOption),
    ("X023", Category::Portability, Severity::Warning, SubstringExpansion),
    ("X024", Category::Portability, Severity::Warning, CaseModificationExpansion),
    ("X025", Category::Portability, Severity::Warning, ReplacementExpansion),
    ("X026", Category::Portability, Severity::Warning, BashFileSlurp),
    ("X027", Category::Portability, Severity::Warning, EchoFlags),
    ("X028", Category::Portability, Severity::Warning, TrLowerRange),
    ("X029", Category::Portability, Severity::Warning, TrUpperRange),
    ("X030", Category::Portability, Severity::Warning, EchoBackslashEscapes),
    ("X031", Category::Portability, Severity::Warning, SourceBuiltinInSh),
    ("X032", Category::Portability, Severity::Warning, PrintfQFormatInSh),
    ("X033", Category::Portability, Severity::Warning, IfElifBashTest),
    ("X035", Category::Portability, Severity::Warning, FunctionParamsInSh),
    ("X037", Category::Portability, Severity::Warning, ExtglobCase),
    ("X048", Category::Portability, Severity::Warning, ExtglobInCasePattern),
    ("X054", Category::Portability, Severity::Warning, ExtglobInSh),
    ("X065", Category::Portability, Severity::Warning, CaretNegationInBracket),
    ("X036", Category::Portability, Severity::Warning, ZshRedirPipe),
    ("X038", Category::Portability, Severity::Warning, ZshBraceIf),
    ("X039", Category::Portability, Severity::Warning, ZshAlwaysBlock),
    ("X040", Category::Portability, Severity::Warning, ArraySubscriptTest),
    ("X041", Category::Portability, Severity::Warning, ArraySubscriptCondition),
    ("X042", Category::Portability, Severity::Warning, SourcedWithArgs),
    ("X043", Category::Portability, Severity::Warning, ZshFlagExpansion),
    ("X044", Category::Portability, Severity::Warning, NestedZshSubstitution),
    ("X045", Category::Portability, Severity::Warning, PlusEqualsAppend),
    ("X051", Category::Portability, Severity::Warning, ZshNestedExpansion),
    ("X047", Category::Portability, Severity::Warning, MultiVarForLoop),
    ("X049", Category::Portability, Severity::Warning, ZshPromptBracket),
    ("X050", Category::Portability, Severity::Warning, CshSyntaxInSh),
    ("X053", Category::Portability, Severity::Warning, ZshAssignmentToZero),
    ("X056", Category::Portability, Severity::Warning, CStyleForInSh),
    ("X055", Category::Portability, Severity::Warning, DollarStringInSh),
    ("X057", Category::Portability, Severity::Warning, LegacyArithmeticInSh),
    ("X062", Category::Portability, Severity::Warning, CStyleForArithmeticInSh),
    ("X071", Category::Portability, Severity::Warning, ArrayKeysInSh),
    ("X081", Category::Portability, Severity::Warning, StarGlobRemovalInSh),
    ("X076", Category::Portability, Severity::Warning, ZshParameterFlag),
    ("X077", Category::Portability, Severity::Warning, NestedDefaultExpansion),
    ("X078", Category::Portability, Severity::Warning, ZshArraySubscriptInCase),
    ("X079", Category::Portability, Severity::Warning, ZshParameterIndexFlag),
    ("X046", Category::Portability, Severity::Warning, ExtglobInTest),
    ("X052", Category::Portability, Severity::Warning, FunctionKeywordInSh),
    ("X058", Category::Portability, Severity::Warning, LexicalComparisonInDoubleBracket),
    ("X059", Category::Portability, Severity::Warning, RegexMatchInSh),
    ("X060", Category::Portability, Severity::Warning, VTestInSh),
    ("X061", Category::Portability, Severity::Warning, ATestInSh),
    ("X063", Category::Portability, Severity::Warning, AmpersandRedirectInSh),
    ("X066", Category::Portability, Severity::Warning, PipeStderrInSh),
    ("X067", Category::Portability, Severity::Warning, HyphenatedFunctionName),
    ("X068", Category::Portability, Severity::Warning, ErrexitTrapInSh),
    ("X069", Category::Portability, Severity::Warning, SignalNameInTrap),
    ("X070", Category::Portability, Severity::Warning, BasePrefixInArithmetic),
    ("X072", Category::Portability, Severity::Warning, UnsetPatternInSh),
    ("X073", Category::Portability, Severity::Warning, OptionTestInSh),
    ("X074", Category::Portability, Severity::Warning, StickyBitTestInSh),
    ("X075", Category::Portability, Severity::Warning, OwnershipTestInSh),
    ("X080", Category::Portability, Severity::Warning, SourceInsideFunctionInSh),
    ("K001", Category::Security, Severity::Warning, RmGlobOnVariablePath),
    ("K002", Category::Security, Severity::Warning, SshLocalExpansion),
    ("K003", Category::Security, Severity::Warning, EvalOnArray),
    ("K004", Category::Security, Severity::Warning, FindExecDirWithShell),
    ("K006", Category::Security, Severity::Warning, RmRootishTarget),
    (
        "K007",
        Category::Security,
        Severity::Warning,
        ChmodWorldWritableSensitivePath
    ),
    ("K008", Category::Security, Severity::Warning, ForkBombPattern),
    ("S001", Category::Style, Severity::Warning, UnquotedExpansion),
    ("S002", Category::Style, Severity::Warning, ReadWithoutRaw),
    ("S003", Category::Style, Severity::Warning, LoopFromCommandOutput),
    ("S004", Category::Style, Severity::Warning, UnquotedCommandSubstitution),
    ("S005", Category::Style, Severity::Warning, LegacyBackticks),
    ("S006", Category::Style, Severity::Warning, LegacyArithmeticExpansion),
    ("S007", Category::Style, Severity::Warning, PrintfFormatVariable),
    ("S008", Category::Style, Severity::Warning, UnquotedArrayExpansion),
    ("S009", Category::Style, Severity::Warning, EchoedCommandSubstitution),
    ("S010", Category::Style, Severity::Warning, ExportCommandSubstitution),
    ("S011", Category::Style, Severity::Warning, CompoundTestOperator),
    ("S012", Category::Style, Severity::Warning, PsGrepPipeline),
    ("S013", Category::Style, Severity::Warning, LsGrepPipeline),
    ("S014", Category::Style, Severity::Warning, UnquotedDollarStar),
    ("S015", Category::Style, Severity::Warning, QuotedDollarStarLoop),
    ("S017", Category::Style, Severity::Warning, UnquotedArraySplit),
    ("S018", Category::Style, Severity::Warning, CommandOutputArraySplit),
    ("S021", Category::Style, Severity::Warning, PositionalArgsInString),
    ("S020", Category::Style, Severity::Warning, SingleIterationLoop),
    ("S032", Category::Style, Severity::Warning, BareCommandNameAssignment),
    ("S036", Category::Style, Severity::Warning, BareRead),
    ("S037", Category::Style, Severity::Warning, RedundantSpacesInEcho),
    ("S038", Category::Style, Severity::Warning, RedundantReturnStatus),
    ("S044", Category::Style, Severity::Warning, EchoToSedSubstitution),
    ("S050", Category::Style, Severity::Hint, UnquotedWordBetweenQuotes),
    ("S051", Category::Style, Severity::Warning, UnquotedTrClass),
    ("S052", Category::Style, Severity::Warning, UnquotedVariableInTest),
    ("S054", Category::Style, Severity::Warning, SuWithoutFlag),
    (
        "S055",
        Category::Style,
        Severity::Warning,
        GlobAssignedToVariable
    ),
    ("S056", Category::Style, Severity::Warning, CommandSubstitutionInAlias),
    ("S057", Category::Style, Severity::Warning, FunctionInAlias),
    ("S058", Category::Style, Severity::Warning, UnquotedPathInMkdir),
    ("S059", Category::Style, Severity::Warning, DeprecatedTempfileCommand),
    ("S060", Category::Style, Severity::Warning, EgrepDeprecated),
    ("S061", Category::Style, Severity::Warning, FgrepDeprecated),
    ("S062", Category::Style, Severity::Warning, DefaultValueInColonAssign),
    ("S064", Category::Style, Severity::Warning, XargsWithInlineReplace),
    ("S065", Category::Style, Severity::Warning, XPrefixInTest),
    ("S067", Category::Style, Severity::Warning, LeadingGlobInGrepPattern),
    ("S068", Category::Style, Severity::Warning, TrapSignalNumbers),
    ("S069", Category::Style, Severity::Hint, GetoptsInvalidFlagHandler),
    ("S070", Category::Style, Severity::Warning, DoubleQuoteNesting),
    ("S071", Category::Style, Severity::Warning, EnvPrefixCommandOnly),
    ("S076", Category::Style, Severity::Warning, MixedQuoteWord),
    (
        "S077",
        Category::Style,
        Severity::Warning,
        BraceVariableBeforeBracket
    ),
    ("S049", Category::Style, Severity::Warning, UnquotedTrRange),
    ("S046", Category::Style, Severity::Warning, LsPipedToXargs),
    ("S047", Category::Style, Severity::Warning, LsInSubstitution),
    (
        "S016",
        Category::Style,
        Severity::Warning,
        EchoInsideCommandSubstitution
    ),
    ("S019", Category::Style, Severity::Warning, GrepOutputInTest),
    ("S022", Category::Style, Severity::Hint, AvoidLetBuiltin),
    ("S033", Category::Style, Severity::Warning, EchoHereDoc),
    ("S034", Category::Style, Severity::Warning, ArrayIndexArithmetic),
    ("S035", Category::Style, Severity::Warning, ArithmeticScoreLine),
    ("S045", Category::Style, Severity::Warning, DollarInArithmetic),
    ("S023", Category::Style, Severity::Warning, EscapedUnderscore),
    ("S024", Category::Style, Severity::Warning, SingleQuoteBackslash),
    ("S025", Category::Style, Severity::Warning, LiteralBackslash),
    ("SH-173", Category::Style, Severity::Warning, BackslashBeforeCommand),
    ("S028", Category::Style, Severity::Warning, SuspectClosingQuote),
    ("S029", Category::Style, Severity::Warning, LiteralBraces),
    ("S030", Category::Style, Severity::Warning, HeredocEndSpace),
    ("S031", Category::Style, Severity::Warning, TrailingDirective),
    (
        "S039",
        Category::Style,
        Severity::Warning,
        LiteralBackslashInSingleQuotes
    ),
    ("S040", Category::Style, Severity::Warning, LiteralControlEscape),
    (
        "S041",
        Category::Style,
        Severity::Warning,
        FunctionBodyWithoutBraces
    ),
    (
        "S066",
        Category::Style,
        Severity::Warning,
        LocalDeclareCombined
    ),
    ("S042", Category::Style, Severity::Warning, IfsEqualsAmbiguity),
    ("S043", Category::Style, Severity::Warning, MissingShebangLine),
    ("S053", Category::Style, Severity::Warning, DuplicateShebangFlag),
    ("S072", Category::Style, Severity::Warning, LinebreakBeforeAnd),
    ("S073", Category::Style, Severity::Warning, SpacedTabstripClose),
    ("S074", Category::Style, Severity::Warning, AmpersandSemicolon),
    ("S075", Category::Style, Severity::Warning, CombineAppends),
    ("S078", Category::Style, Severity::Warning, ShebangShellPolicy),
    ("S079", Category::Style, Severity::Warning, ShebangFormPolicy),
    ("S080", Category::Style, Severity::Warning, ScriptSizeThreshold),
    ("S081", Category::Style, Severity::Warning, MissingFileDescription),
    ("S082", Category::Style, Severity::Warning, TodoFormat),
    ("S083", Category::Style, Severity::Warning, MissingFunctionDoc),
    ("S084", Category::Style, Severity::Warning, FunctionDocContent),
    ("S085", Category::Style, Severity::Warning, MissingMainEntrypoint),
}

pub fn code_to_rule(code: &str) -> Option<Rule> {
    canonical_code_to_rule(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn rule_codes_are_unique() {
        let codes = Rule::iter().map(Rule::code).collect::<BTreeSet<_>>();
        assert_eq!(codes.len(), Rule::COUNT);
    }

    #[test]
    fn canonical_codes_resolve_and_legacy_aliases_do_not() {
        assert_eq!(code_to_rule("C001"), Some(Rule::UnusedAssignment));
        assert_eq!(code_to_rule("S001"), Some(Rule::UnquotedExpansion));
        assert_eq!(code_to_rule("S014"), Some(Rule::UnquotedDollarStar));
        assert_eq!(code_to_rule("SH-001"), None);
        assert_eq!(code_to_rule("SH-002"), None);
        assert_eq!(code_to_rule("SH-003"), None);
    }
}
