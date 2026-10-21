import SwiftUI

/// Phone sign-in: a country and national number, then the six-digit code the
/// server texts. The input and action rows keep their positions between the two
/// steps, with room reserved for the resend control, so nothing jumps.
struct SignInView: View {
    @Environment(AppState.self) private var state
    @Environment(\.dismiss) private var dismiss

    private enum Step: Hashable { case number, code }

    @State private var step = Step.number
    @State private var country = PhoneCountries.current
    @State private var national = ""
    @State private var code = ""
    @State private var working = false
    @State private var error = ""
    @State private var resendIn = 0
    @State private var pickingCountry = false
    /// The number field is UIKit, so its keyboard is driven by a flag of its
    /// own rather than the focus state the code field uses.
    @State private var numberFocused = false
    @FocusState private var focus: Step?

    private var phone: String { PhoneCountries.e164(country, national) }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 22) {
                    header
                    Group {
                        if step == .number { numberField } else { codeField }
                    }
                    .id(step)
                    .disabled(working)
                    .task(id: step) {
                        await Task.yield()
                        if step == .number { numberFocused = true } else { focus = step }
                    }
                    if !error.isEmpty {
                        Text(error)
                            .font(.footnote)
                            .foregroundStyle(Color.danger)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    actions
                    Spacer(minLength: 0)
                    notice
                }
                .padding(.horizontal, 24)
                .padding(.top, 12)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .scrollDismissesKeyboard(.interactively)
            // A top anchor keeps the form still when the keyboard appears.
            .defaultScrollAnchor(.top)
            .background(Color.surface)
            .navigationTitle("Sign In")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .tint(Color.toolbarIcon)
                }
            }
            .sheet(isPresented: $pickingCountry) {
                CountryPicker(selected: $country)
            }
        }
        .interactiveDismissDisabled(working)
        .task(id: resendIn) {
            guard resendIn > 0 else { return }
            try? await Task.sleep(for: .seconds(1))
            resendIn -= 1
        }
    }

    // MARK: Pieces

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(step == .number ? "What is your number?" : "Enter your code")
                .font(.display(26, relativeTo: .title))
                .foregroundStyle(Color.ink)
            Text(
                step == .number
                    ? "We’ll text you a sign-in code."
                    : "We sent a code to \(country.dialing) \(national)."
            )
            .font(.subheadline)
            .foregroundStyle(Color.inkSecondary)
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var numberField: some View {
        HStack(spacing: 10) {
            Button {
                pickingCountry = true
            } label: {
                HStack(spacing: 4) {
                    Text(country.flag)
                    Text(country.dialing).font(.body.monospacedDigit())
                    Image(systemName: "chevron.down").font(.caption2)
                }
                .foregroundStyle(Color.ink)
                .padding(.horizontal, 12)
                .frame(minHeight: fieldHeight)
                .background(Color.card, in: .rect(cornerRadius: 16, style: .continuous))
                .overlay { RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Color.hairline, lineWidth: 1) }
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Country: \(country.name), \(country.dialing)")
            .accessibilityHint("Choose another country")

            PhoneNumberField(national: $national, country: $country, focused: $numberFocused)
                .frame(maxWidth: .infinity)
                .padding(.horizontal, 12)
                .frame(minHeight: fieldHeight)
                .background(Color.card, in: .rect(cornerRadius: 16, style: .continuous))
                .overlay { RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Color.hairline, lineWidth: 1) }
        }
    }

    private var codeField: some View {
        TextField("000000", text: $code)
            .keyboardType(.numberPad)
            .textContentType(.oneTimeCode)
            .font(.title2.monospacedDigit().weight(.semibold))
            .multilineTextAlignment(.center)
            .focused($focus, equals: .code)
            .frame(minHeight: fieldHeight)
            .background(Color.card, in: .rect(cornerRadius: 16, style: .continuous))
                .overlay { RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Color.hairline, lineWidth: 1) }
            .accessibilityLabel("Sign-in code")
            .onChange(of: code) { _, value in
                code = String(value.filter { $0.isASCII && $0.isNumber }.prefix(6))
                if code.count == 6 { Task { await verify() } }
            }
    }

    private var actions: some View {
        VStack(spacing: 10) {
            Button {
                Task { step == .number ? await sendCode() : await verify() }
            } label: {
                Group {
                    if working {
                        ProgressView().tint(Color.onBrand)
                    } else {
                        Text(step == .number ? "Send Code" : "Sign In")
                            .font(.body.weight(.semibold))
                    }
                }
            }
            .buttonStyle(.pill)
            .disabled(working || !canContinue)

            // Reserved whether or not the resend control is showing yet.
            Group {
                if step == .code {
                    HStack(spacing: 16) {
                        Button(resendIn > 0 ? "Resend in \(resendIn)s" : "Resend Code") {
                            Task { await sendCode(resending: true) }
                        }
                        .disabled(resendIn > 0 || working)
                        // One label for VoiceOver, with the countdown as its value.
                        .accessibilityLabel("Resend Code")
                        .accessibilityValue(resendIn > 0 ? "in \(resendIn) seconds" : "")
                        Button("Change Number") {
                            step = .number
                            code = ""
                            error = ""
                        }
                    }
                    .font(.footnote)
                } else {
                    Color.clear.frame(height: 20)
                }
            }
            .frame(minHeight: 20)
        }
    }

    /// The input rows, sized for the text they hold.
    @ScaledMetric(relativeTo: .body) private var fieldHeight = 48.0

    private var notice: some View {
        Text(
            """
            By continuing you agree to receive an SMS with a sign-in code. \
            Message and data rates may apply. Honorum keeps the number, the time \
            you confirmed, and the notice version for one year. SMS is the only \
            way to sign in: losing the number can mean losing the account.
            """
        )
        .font(.caption2)
        .foregroundStyle(Color.inkMuted)
        .fixedSize(horizontal: false, vertical: true)
        .padding(.bottom, 20)
    }

    private var canContinue: Bool {
        step == .number ? PhoneCountries.isPlausible(country, national) : code.count == 6
    }

    // MARK: Actions

    private func sendCode(resending: Bool = false) async {
        guard !working else { return }
        working = true
        error = ""
        defer { working = false }
        do {
            let wait = try await state.requestSignInCode(phone: phone)
            resendIn = wait
            if !resending {
                step = .code
                numberFocused = false
            }
        } catch {
            // A refused request says when it may be repeated; the countdown
            // keeps the button from asking again before then.
            if case HonorumAPI.Failure.rateLimited(_, let after) = error { resendIn = max(after, 1) }
            fail(with: error)
        }
    }

    private func fail(with error: any Error) {
        self.error = message(for: error)
        AccessibilityNotification.Announcement(self.error).post()
    }

    private func verify() async {
        guard !working else { return }
        working = true
        error = ""
        defer { working = false }
        do {
            try await state.verifySignIn(phone: phone, code: code)
            dismiss()
        } catch {
            fail(with: error)
            code = ""
        }
    }

    private func message(for error: any Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? "That did not work. Try again."
    }
}

/// The country list, searchable by name or dialing code.
struct CountryPicker: View {
    @Binding var selected: PhoneCountries.Country
    @Environment(\.dismiss) private var dismiss
    @State private var query = ""

    var body: some View {
        NavigationStack {
            AppList {
                ForEach(matches) { country in
                    Button {
                        selected = country
                        dismiss()
                    } label: {
                        HStack {
                            Text(country.flag)
                            Text(country.name).foregroundStyle(Color.ink)
                            Spacer()
                            Text(country.dialing)
                                .font(.body.monospacedDigit())
                                .foregroundStyle(Color.inkMuted)
                            if country.id == selected.id {
                                Image(systemName: "checkmark").foregroundStyle(Color.brand)
                            }
                        }
                    }
                }
            }
            .listStyle(.plain)
            .searchable(text: $query, prompt: "Country or code")
            .navigationTitle("Country")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .tint(Color.toolbarIcon)
                }
            }
        }
    }

    private var matches: [PhoneCountries.Country] {
        let needle = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !needle.isEmpty else { return PhoneCountries.all }
        return PhoneCountries.all.filter {
            $0.name.lowercased().contains(needle) || $0.dialing.contains(needle)
                || $0.id.lowercased() == needle
        }
    }
}

#if DEBUG
#Preview("Sign In") {
    SignInView()
        .environment(AppState.preview())
        .tint(.brand)
}
#endif
